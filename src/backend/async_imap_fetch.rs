//! Compatibility fallback that talks IMAP via `async-imap` (RFC 3501 parser)
//! instead of the `imap-client` crate wired through `email-lib`.
//!
//! Motivation: `imap-client` 0.3.1 (as of 2026-06) has a strict, tokenizing
//! parser that treats some responses from real-world servers (e.g. 263.net /
//! Postfix) as `MalformedMessage` and silently drops them
//! (`src/tasks/mod.rs:143` — "HACK: skip bad fetches, improve me").
//!
//! `async-imap` uses `imap-proto`'s more lenient nom parser, which handles
//! those responses. We keep this module narrowly scoped: it only fetches raw
//! RFC-822 bytes and hands them back for the CLI layer to parse with
//! `mail_parser`. All state mutation (flags, move, delete, send) continues to
//! go through email-lib.
//!
//! ## Keychain interaction (macOS)
//!
//! Every `KeyringEntry::get_secret()` call on macOS can trigger a "always
//! allow?" prompt in Keychain Access. If we called it once per async-imap
//! session (e.g. per parallel worker), the user would see multiple prompts
//! and the program would appear hung while waiting for input. To fix this,
//! callers **must resolve credentials exactly once** via [`ImapCreds::resolve`]
//! and thread the resulting struct into every fetch call.

use std::collections::HashMap;
use std::sync::Arc;

use async_imap::Client;
use async_imap::types::Flag as AsyncFlag;
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::{debug, info, warn};

use crate::config::account::{AccountConfig, Encryption};
use crate::credentials;
use crate::error::{Error, Result};

/// Default concurrency for the parallel batch fetch. 3 is a good balance:
/// meaningful speedup on slow servers, low enough to stay under most
/// per-account IMAP connection caps. Because we pre-resolve the password
/// exactly once (see [`ImapCreds::resolve`]), spawning multiple workers no
/// longer causes multiple keyring / macOS Keychain prompts.
const DEFAULT_CONCURRENCY: usize = 3;

/// Pre-resolved IMAP credentials — password is read from the OS keyring once
/// by the caller and shared into every async-imap session. Critical on macOS,
/// where each `keyring::get_secret()` may trigger a Keychain authorization
/// prompt; reading once means the user sees at most one prompt per run.
#[derive(Clone, Debug)]
pub struct ImapCreds {
    pub host: String,
    pub port: u16,
    pub encryption: Encryption,
    pub login: String,
    pub password: String,
}

impl ImapCreds {
    /// Read the password from the OS keyring exactly once for this account.
    pub async fn resolve(account_name: &str, cfg: &AccountConfig) -> Result<Self> {
        let password = credentials::load(&credentials::imap_key(account_name)).await?;
        Ok(Self {
            host: cfg.imap.host.clone(),
            port: cfg.imap.port,
            encryption: cfg.imap.encryption,
            login: cfg.imap.login.clone(),
            password,
        })
    }

    fn require_tls(&self) -> Result<()> {
        match self.encryption {
            Encryption::Tls => Ok(()),
            Encryption::Starttls => Err(Error::NotImplemented(
                "STARTTLS in async-imap path (v0.1: use tls only)",
            )),
            Encryption::None => Err(Error::Config(
                "plaintext IMAP not supported in async-imap path".into(),
            )),
        }
    }
}

/// A message pulled directly via async-imap. Owns the raw RFC-822 bytes and the
/// server-side flag state; callers parse the body themselves with `mail_parser`.
pub struct FetchedMsg {
    pub uid: String,
    pub is_seen: bool,
    pub raw_body: Vec<u8>,
}

/// Fetch one UID's body via async-imap. Convenience wrapper for the
/// single-message `message read` path.
pub async fn fetch_raw_by_uid(
    creds: &ImapCreds,
    folder: &str,
    uid: &str,
) -> Result<Vec<u8>> {
    let map = fetch_raw_by_uids(creds, folder, &[uid.to_string()]).await?;
    map.into_iter()
        .next()
        .map(|(_, v)| v)
        .ok_or_else(|| Error::Transient(format!("async-imap: no body for uid {uid}")))
}

/// Batch-fetch several UIDs in a single IMAP session (one TCP + TLS + LOGIN + SELECT).
/// Returns `uid → raw RFC-822 bytes`. UIDs the server returned nothing for are
/// simply absent from the map — callers decide how to handle that.
pub async fn fetch_raw_by_uids(
    creds: &ImapCreds,
    folder: &str,
    uids: &[String],
) -> Result<HashMap<String, Vec<u8>>> {
    if uids.is_empty() {
        return Ok(HashMap::new());
    }
    creds.require_tls()?;

    debug!(
        host = %creds.host,
        port = creds.port,
        login = %creds.login,
        folder,
        n = uids.len(),
        "async-imap batch fetch"
    );

    let mut session = open_session(creds).await?;
    session
        .select(folder)
        .await
        .map_err(|e| Error::Transient(format!("select `{folder}`: {e}")))?;

    // "uid1,uid2,uid3" — IMAP UID set. Sorted for determinism.
    let mut sorted = uids.to_vec();
    sorted.sort();
    sorted.dedup();
    let uid_set = sorted.join(",");

    let mut fetches = session
        .uid_fetch(uid_set, "BODY.PEEK[]")
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch: {e}")))?;

    let mut out: HashMap<String, Vec<u8>> = HashMap::with_capacity(sorted.len());
    while let Some(fetch) = fetches
        .try_next()
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch stream: {e}")))?
    {
        let uid = fetch
            .uid
            .map(|u| u.to_string())
            .unwrap_or_else(|| "?".into());
        if let Some(body) = fetch.body() {
            out.insert(uid, body.to_vec());
        }
    }
    drop(fetches);
    let _ = session.logout().await;

    Ok(out)
}

/// One-shot IMAP SEARCH + batch FETCH via async-imap. Used when
/// `email-lib`'s SEARCH path is broken (e.g. 263.net Postfix).
///
/// Returns messages newest-UID-first, capped at `limit`. Empty vec if no matches.
pub async fn search_and_fetch(
    creds: &ImapCreds,
    folder: &str,
    search_criteria: &str,
    limit: usize,
) -> Result<Vec<FetchedMsg>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    creds.require_tls()?;

    debug!(
        host = %creds.host,
        port = creds.port,
        folder,
        criteria = %search_criteria,
        "async-imap search+fetch"
    );

    let mut session = open_session(creds).await?;
    session
        .select(folder)
        .await
        .map_err(|e| Error::Transient(format!("select `{folder}`: {e}")))?;

    let uids = session
        .uid_search(search_criteria)
        .await
        .map_err(|e| Error::Transient(format!("uid_search `{search_criteria}`: {e}")))?;
    let mut uid_list: Vec<u32> = uids.into_iter().collect();
    uid_list.sort_by(|a, b| b.cmp(a));
    uid_list.truncate(limit);
    if uid_list.is_empty() {
        let _ = session.logout().await;
        return Ok(Vec::new());
    }

    let uid_set = uid_list
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let mut fetches = session
        .uid_fetch(uid_set, "(FLAGS BODY.PEEK[])")
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch: {e}")))?;

    let mut out: Vec<FetchedMsg> = Vec::with_capacity(uid_list.len());
    while let Some(fetch) = fetches
        .try_next()
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch stream: {e}")))?
    {
        let uid = match fetch.uid {
            Some(u) => u.to_string(),
            None => continue,
        };
        let is_seen = fetch.flags().any(|f| matches!(f, AsyncFlag::Seen));
        let raw_body = match fetch.body() {
            Some(b) => b.to_vec(),
            None => continue,
        };
        out.push(FetchedMsg {
            uid,
            is_seen,
            raw_body,
        });
    }
    drop(fetches);
    let _ = session.logout().await;

    out.sort_by(|a, b| {
        b.uid
            .parse::<u32>()
            .unwrap_or(0)
            .cmp(&a.uid.parse::<u32>().unwrap_or(0))
    });
    Ok(out)
}

/// Batch-fetch bodies in parallel across multiple async-imap sessions. Callers
/// pre-resolve credentials so the password is read from keyring exactly once
/// regardless of `concurrency` — critical to keep macOS Keychain from prompting
/// multiple times.
pub async fn fetch_raw_by_uids_parallel(
    creds: &ImapCreds,
    folder: &str,
    uids: &[String],
    concurrency: Option<usize>,
) -> Result<HashMap<String, Vec<u8>>> {
    if uids.is_empty() {
        return Ok(HashMap::new());
    }
    let concurrency = concurrency.unwrap_or(DEFAULT_CONCURRENCY).max(1).min(uids.len());
    if concurrency == 1 {
        return fetch_raw_by_uids(creds, folder, uids).await;
    }

    let chunk_size = uids.len().div_ceil(concurrency);
    let chunks: Vec<Vec<String>> = uids
        .chunks(chunk_size)
        .map(<[String]>::to_vec)
        .collect();

    info!(
        n = uids.len(),
        workers = chunks.len(),
        "parallel body fetch: {} UIDs across {} workers",
        uids.len(),
        chunks.len()
    );

    // Share the resolved credentials by Arc — no additional keyring reads.
    let creds = Arc::new(creds.clone());
    let folder = Arc::new(folder.to_string());

    let mut handles = Vec::with_capacity(chunks.len());
    for (i, chunk) in chunks.into_iter().enumerate() {
        let creds = Arc::clone(&creds);
        let folder = Arc::clone(&folder);
        handles.push(tokio::spawn(async move {
            debug!(worker = i, n = chunk.len(), "worker started");
            let started = std::time::Instant::now();
            let out = fetch_raw_by_uids(&creds, &folder, &chunk).await;
            let elapsed = started.elapsed();
            match &out {
                Ok(map) => info!(
                    worker = i,
                    fetched = map.len(),
                    duration_ms = elapsed.as_millis() as u64,
                    "worker done"
                ),
                Err(e) => warn!(worker = i, error = %e, "worker failed"),
            }
            out
        }));
    }

    let mut combined = HashMap::with_capacity(uids.len());
    for (i, h) in handles.into_iter().enumerate() {
        match h.await {
            Ok(Ok(map)) => combined.extend(map),
            Ok(Err(e)) => warn!(worker = i, error = %e, "chunk error"),
            Err(e) => warn!(worker = i, error = %e, "chunk panicked"),
        }
    }
    Ok(combined)
}

/// Open a fresh TCP + TLS + LOGIN async-imap session. No keyring access here.
async fn open_session(
    creds: &ImapCreds,
) -> Result<async_imap::Session<tokio_rustls::client::TlsStream<TcpStream>>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));

    let tcp = TcpStream::connect((creds.host.as_str(), creds.port))
        .await
        .map_err(|e| Error::Transient(format!("tcp connect {}:{}: {e}", creds.host, creds.port)))?;
    let server_name = ServerName::try_from(creds.host.clone())
        .map_err(|e| Error::Config(format!("invalid hostname `{}`: {e}", creds.host)))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Transient(format!("tls handshake: {e}")))?;

    let mut client = Client::new(tls);
    let _greeting = client
        .read_response()
        .await
        .map_err(|e| Error::Transient(format!("imap greeting: {e}")))?
        .ok_or_else(|| Error::Transient("imap greeting: EOF".into()))?;

    let session = client
        .login(&creds.login, &creds.password)
        .await
        .map_err(|(e, _)| Error::Config(format!("imap login: {e}")))?;
    Ok(session)
}
