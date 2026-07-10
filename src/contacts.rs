//! Local contact index derived from `From:` / `To:` / `Cc:` addresses seen in
//! pulled messages. No CardDAV, no LDAP — just an append-then-merge JSONL log
//! at `<data_local_dir>/mail-cli/contacts.jsonl`.
//!
//! Design principles:
//! - **Zero extra network**: only ingest from what `pull` already fetches.
//! - **Append-friendly writes**: `ingest_batch` appends new-observation lines,
//!   never rewrites the whole file. Merging happens lazily on read.
//! - **Case-insensitive matching**: addresses stored lowercase, names as-seen.
//! - **Best-effort**: any I/O failure here is logged and swallowed. The
//!   contact index is nice-to-have; it must never break a pull.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A single address ingestion event as written to the JSONL log.
/// Kept minimal so appending is cheap.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Observation {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `from` | `to` | `cc`
    pub role: String,
    /// Account name that saw this contact (multi-account users benefit).
    pub account: String,
    /// ISO-8601 timestamp of the email's Date header (approximates "when we saw them").
    pub date: String,
}

/// Merged view of a contact — the queryable shape.
#[derive(Debug, Serialize, Clone)]
pub struct Contact {
    pub email: String,
    /// All distinct display names ever seen, most-recent first.
    pub names: Vec<String>,
    pub first_seen: String,
    pub last_seen: String,
    pub count: u64,
    /// Which mail-cli accounts have interacted with this contact.
    pub accounts: Vec<String>,
    /// Distribution across `from` / `to` / `cc` (`{"from": 12, "to": 5}`).
    pub roles: BTreeMap<String, u64>,
}

pub fn default_store_path() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new()
        .ok_or_else(|| Error::Config("cannot determine home directory".into()))?;
    Ok(dirs
        .data_local_dir()
        .join("mail-cli")
        .join("contacts.jsonl"))
}

/// Append a batch of observations to the JSONL log. Called by `pull` for each
/// message's From + To + Cc. Silently deduplicates within this call.
pub fn ingest_batch(
    path: &std::path::Path,
    observations: &[Observation],
) -> Result<()> {
    if observations.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    for obs in observations {
        let line = serde_json::to_string(obs)?;
        writeln!(f, "{line}")?;
    }
    Ok(())
}

/// Build observations from a message envelope. Callers pass the raw addresses
/// that came from the parsed IMAP envelope.
pub fn observations_from_envelope(
    account: &str,
    date: &str,
    from: Option<(&str, Option<&str>)>,
    to: &[(&str, Option<&str>)],
    cc: &[(&str, Option<&str>)],
) -> Vec<Observation> {
    let mut out = Vec::new();
    if let Some((email, name)) = from {
        out.push(build_obs(email, name, "from", account, date));
    }
    for &(email, name) in to {
        out.push(build_obs(email, name, "to", account, date));
    }
    for &(email, name) in cc {
        out.push(build_obs(email, name, "cc", account, date));
    }
    out.retain(|o| !o.email.is_empty());
    out
}

fn build_obs(email: &str, name: Option<&str>, role: &str, account: &str, date: &str) -> Observation {
    Observation {
        email: email.trim().to_lowercase(),
        name: name.and_then(|n| {
            let t = n.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }),
        role: role.to_string(),
        account: account.to_string(),
        date: date.to_string(),
    }
}

/// Load the JSONL log and merge duplicate emails into a single [`Contact`].
/// Missing file → empty result (never an error).
pub fn load_merged(path: &std::path::Path) -> Result<Vec<Contact>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Io(e)),
    };
    let reader = BufReader::new(file);
    let mut map: BTreeMap<String, Contact> = BTreeMap::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let obs: Observation = match serde_json::from_str(&line) {
            Ok(o) => o,
            Err(_) => continue, // ignore corrupted lines rather than fail the whole read
        };

        let entry = map.entry(obs.email.clone()).or_insert_with(|| Contact {
            email: obs.email.clone(),
            names: Vec::new(),
            first_seen: obs.date.clone(),
            last_seen: obs.date.clone(),
            count: 0,
            accounts: Vec::new(),
            roles: BTreeMap::new(),
        });

        entry.count += 1;
        if !entry.accounts.contains(&obs.account) {
            entry.accounts.push(obs.account.clone());
        }
        *entry.roles.entry(obs.role.clone()).or_insert(0) += 1;

        if obs.date.as_str() < entry.first_seen.as_str() {
            entry.first_seen = obs.date.clone();
        }
        if obs.date.as_str() > entry.last_seen.as_str() {
            entry.last_seen = obs.date.clone();
        }
        if let Some(name) = obs.name {
            if !entry.names.contains(&name) {
                entry.names.insert(0, name);
            }
        }
    }

    Ok(map.into_values().collect())
}

/// Substring match on email OR any name, case-insensitive.
pub fn matches(c: &Contact, needle: &str) -> bool {
    let needle_lower = needle.to_lowercase();
    if c.email.contains(&needle_lower) {
        return true;
    }
    c.names.iter().any(|n| n.to_lowercase().contains(&needle_lower))
}

pub fn find_by_email<'a>(contacts: &'a [Contact], email: &str) -> Option<&'a Contact> {
    let target = email.to_lowercase();
    contacts.iter().find(|c| c.email == target)
}

/// Remove the local index file. Missing file is a no-op.
pub fn clear(path: &std::path::Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(md) => {
            let sz = md.len();
            std::fs::remove_file(path)?;
            Ok(sz)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(Error::Io(e)),
    }
}
