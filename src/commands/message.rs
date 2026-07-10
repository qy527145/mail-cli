use std::io::Read;
use std::path::{Path, PathBuf};

use email::envelope::flag::{Flag, Flags};
use email::envelope::flag::add::AddFlags;
use email::envelope::flag::remove::RemoveFlags;
use email::envelope::get::GetEnvelope;
use email::envelope::list::{ListEnvelopes, ListEnvelopesOptions};
use email::envelope::{Envelope as EmailEnvelope, Id, SingleId};
use email::imap::ImapContext;
use email::message::add::AddMessage;
use email::message::delete::DeleteMessages;
use email::message::get::GetMessages;
use email::message::peek::PeekMessages;
use email::message::r#move::MoveMessages;
use email::message::send::SendMessage;
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address as MbAddress;
use mail_parser::MimeHeaders;
use serde_json::json;
use tracing::warn;

use crate::backend::{AccountHandle, convert};
use crate::cli::{
    GlobalArgs, MessageArchiveArgs, MessageCommand, MessageDeleteArgs, MessageFlagArgs,
    MessageFormat, MessageListArgs, MessagePullArgs, MessageReadArgs, MessageReplyArgs,
    MessageSendArgs, PullBodyFormat,
};
use crate::config::ConfigFile;
use crate::error::{Error, Result};
use crate::html;
use crate::output::envelope::{EnvelopeList, Pagination};
use crate::output::message::{Message as MessageDto, wrap_untrusted};
use crate::output::{OutputFormat, emit};
use crate::safety;

pub async fn run(cmd: MessageCommand, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        MessageCommand::List(args) => list(args, global, fmt).await,
        MessageCommand::Read(args) => read(args, global, fmt).await,
        MessageCommand::Pull(args) => pull(args, global, fmt).await,
        MessageCommand::Send(args) => send(args, global, fmt).await,
        MessageCommand::Reply(args) => reply(args, global, fmt).await,
        MessageCommand::Flag(args) => flag(args, global, fmt).await,
        MessageCommand::Archive(args) => archive(args, global, fmt).await,
        MessageCommand::Delete(args) => delete(args, global, fmt).await,
    }
}

fn load_account(global: &GlobalArgs) -> Result<AccountHandle> {
    let path = ConfigFile::resolve_path(global.config.as_ref())?;
    let cfg = ConfigFile::load(&path)?;
    let name = cfg
        .resolve_account_name(global.account.as_deref())?
        .to_string();
    let acc_cfg = cfg.account(&name)?.clone();
    Ok(AccountHandle::new(name, acc_cfg))
}

async fn list(args: MessageListArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let opts = ListEnvelopesOptions {
        page: args.page as usize,
        page_size: args.limit as usize,
        query: None,
    };
    let envs = imap
        .list_envelopes(&args.folder, opts)
        .await
        .map_err(|e| Error::Transient(format!("list_envelopes: {e}")))?;

    let items: Vec<_> = envs.iter().map(convert::convert_envelope).collect();
    let out = EnvelopeList {
        envelopes: items,
        pagination: Pagination {
            page: args.page,
            page_size: args.limit,
            total_estimate: None,
        },
    };
    emit(&out, fmt)?;
    Ok(())
}

async fn read(args: MessageReadArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    if matches!(args.format, MessageFormat::Raw) {
        return Err(Error::NotImplemented("--format raw (M5, needs base64)"));
    }

    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let single_id: SingleId = args.id.clone().into();
    let id: Id = Id::single(single_id.clone());

    let envelope = imap
        .get_envelope(&args.folder, &single_id)
        .await
        .map_err(|e| Error::Transient(format!("get_envelope: {e}")))?;

    let messages = if args.mark_read {
        imap.get_messages(&args.folder, &id).await
    } else {
        imap.peek_messages(&args.folder, &id).await
    }
    .map_err(|e| Error::Transient(format!("fetch message: {e}")))?;

    // Try the fast path (email-lib). If it returns empty even though get_envelope
    // succeeded, that indicates an imap-client 0.3.1 parser miss (e.g. 263.net).
    // Fall back to async-imap which has a more lenient parser.
    let (body_text, html_stripped, remote, used_fallback) = if let Some(msg) = messages.first() {
        let parsed = msg
            .parsed()
            .map_err(|e| Error::Transient(format!("parse message: {e}")))?;
        let (b, h, r) = extract_body(parsed);
        (b, h, r, false)
    } else {
        tracing::warn!(
            id = %args.id,
            "email-lib returned empty body (likely imap-client parser miss); \
             falling back to async-imap"
        );
        let creds = crate::backend::async_imap_fetch::ImapCreds::resolve(
            &account.name,
            &account.cfg,
        )
        .await?;
        let raw = crate::backend::async_imap_fetch::fetch_raw_by_uid(
            &creds,
            &args.folder,
            &args.id,
        )
        .await?;
        let parsed = mail_parser::MessageParser::default()
            .parse(&raw)
            .ok_or_else(|| Error::Transient("failed to parse RFC822 from fallback".into()))?;
        let (b, h, r) = extract_body(&parsed);
        (b, h, r, true)
    };

    let mut dto = MessageDto {
        envelope: convert::convert_envelope(&envelope),
        body_text: wrap_untrusted(&args.id, &envelope.from.addr, &body_text),
        html_stripped,
        remote_resources_blocked: remote,
        attachments: vec![],
    };
    if used_fallback {
        // Non-breaking: append a subtle marker to help agents/debuggers know which path was taken.
        dto.body_text.push_str("\n<!-- fetched via async-imap fallback -->");
    }
    emit(&dto, fmt)?;
    Ok(())
}

fn extract_body(parsed: &mail_parser::Message<'_>) -> (String, bool, u32) {
    if let Some(text) = parsed.body_text(0) {
        return (text.into_owned(), false, 0);
    }
    if let Some(html) = parsed.body_html(0) {
        let (text, remote) = html::html_to_text(&html, 80);
        return (text, true, remote);
    }
    (String::new(), false, 0)
}

fn load_body(spec: &str) -> Result<String> {
    if spec == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        Ok(buf)
    } else {
        Ok(std::fs::read_to_string(Path::new(spec))?)
    }
}

fn build_mime(
    account: &AccountHandle,
    to: &[String],
    cc: &[String],
    bcc: &[String],
    subject: &str,
    body: &str,
    in_reply_to: Option<&str>,
    references: Option<&str>,
    attachments: &[std::path::PathBuf],
) -> Result<Vec<u8>> {
    let mut b = MessageBuilder::new()
        .from(account.cfg.email.as_str())
        .subject(subject)
        .text_body(body.to_string())
        .date(chrono::Utc::now().timestamp());

    if !to.is_empty() {
        let list: Vec<MbAddress> = to
            .iter()
            .map(|s| MbAddress::new_address(None::<String>, s.clone()))
            .collect();
        b = b.to(list);
    }
    if !cc.is_empty() {
        let list: Vec<MbAddress> = cc
            .iter()
            .map(|s| MbAddress::new_address(None::<String>, s.clone()))
            .collect();
        b = b.cc(list);
    }
    if !bcc.is_empty() {
        let list: Vec<MbAddress> = bcc
            .iter()
            .map(|s| MbAddress::new_address(None::<String>, s.clone()))
            .collect();
        b = b.bcc(list);
    }

    if let Some(mid) = in_reply_to {
        b = b.in_reply_to(mid.to_string());
    }
    if let Some(refs) = references {
        b = b.references(refs.to_string());
    }

    for path in attachments {
        let bytes = std::fs::read(path)?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("attachment")
            .to_string();
        b = b.attachment("application/octet-stream", filename, bytes);
    }

    b.write_to_vec()
        .map_err(|e| Error::Transient(format!("mime build: {e}")))
}

/// Build an IMAP SEARCH criteria string for use with async-imap `uid_search`.
/// Combines `UNSEEN` (if we're filtering unread) with `SINCE <date>` (if we have
/// a date cutoff). Returns `"ALL"` if no filters apply.
fn build_imap_search_criteria(unread_only: bool, since: Option<chrono::NaiveDate>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if unread_only {
        parts.push("UNSEEN".into());
    }
    if let Some(d) = since {
        // IMAP wire date: `1-Jan-2026`, locale-independent.
        parts.push(format!("SINCE {}", d.format("%-d-%b-%Y")));
    }
    if parts.is_empty() {
        "ALL".into()
    } else {
        parts.join(" ")
    }
}

/// Synthesize an `email::envelope::Envelope` from raw RFC-822 bytes when we
/// took the async-imap SEARCH shortcut. Fills the fields the rest of pull expects.
fn synth_envelope_from_raw(
    uid: &str,
    is_seen: bool,
    raw: &[u8],
) -> Option<email::envelope::Envelope> {
    use email::envelope::address::Address as EmailAddr;
    let parsed = mail_parser::MessageParser::default().parse(raw)?;

    let mut flags = Flags::default();
    if is_seen {
        flags.insert(Flag::Seen);
    }

    let from = parsed
        .from()
        .and_then(mail_parser::Address::first)
        .map(|a| EmailAddr {
            name: a.name().map(str::to_string),
            addr: a.address().unwrap_or("").to_string(),
        })
        .unwrap_or_default();
    let to = parsed
        .to()
        .and_then(mail_parser::Address::first)
        .map(|a| EmailAddr {
            name: a.name().map(str::to_string),
            addr: a.address().unwrap_or("").to_string(),
        })
        .unwrap_or_default();

    let date = parsed
        .date()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(&d.to_rfc3339()).ok())
        .unwrap_or_else(|| {
            chrono::DateTime::from_naive_utc_and_offset(
                chrono::NaiveDateTime::default(),
                chrono::FixedOffset::east_opt(0).unwrap(),
            )
        });

    Some(email::envelope::Envelope {
        id: uid.to_string(),
        message_id: parsed.message_id().unwrap_or("").to_string(),
        in_reply_to: None,
        flags,
        from,
        to,
        subject: parsed.subject().unwrap_or("").to_string(),
        date,
        has_attachment: parsed.attachment_count() > 0,
    })
}

fn parse_since_date(s: &str) -> Result<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| Error::Input(format!("--since must be YYYY-MM-DD: {e}")))
}

/// Parse a duration like `30m`, `2h`, `7d` → NaiveDate cutoff (UTC-relative).
fn parse_max_age(s: &str) -> Result<chrono::NaiveDate> {
    let (num_part, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| Error::Input(format!("--max-age missing unit (m/h/d) in {s:?}")))?,
    );
    let n: i64 = num_part
        .parse()
        .map_err(|_| Error::Input(format!("--max-age not a number: {num_part:?}")))?;
    let duration = match unit {
        "m" => chrono::Duration::minutes(n),
        "h" => chrono::Duration::hours(n),
        "d" => chrono::Duration::days(n),
        _ => {
            return Err(Error::Input(format!(
                "--max-age unit must be m|h|d (got {unit:?})"
            )));
        }
    };
    let cutoff = chrono::Utc::now() - duration;
    Ok(cutoff.date_naive())
}

/// Owned attachment payload extracted from a parsed message. Keeps a fully-owned
/// copy of the bytes so it can outlive the parsed message.
struct OwnedAttachment {
    filename: Option<String>,
    mime_type: String,
    data: Vec<u8>,
}

fn extract_attachments_owned(parsed: &mail_parser::Message<'_>) -> Vec<OwnedAttachment> {
    parsed
        .attachments()
        .map(|part| OwnedAttachment {
            filename: part.attachment_name().map(str::to_string),
            mime_type: part
                .content_type()
                .map(|ct| match ct.c_subtype.as_ref() {
                    Some(sub) => format!("{}/{}", ct.c_type, sub),
                    None => ct.c_type.to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            data: part.contents().to_vec(),
        })
        .collect()
}

/// Replace filesystem-hostile characters. Empty string turns into "unnamed".
fn sanitize_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.is_empty() {
        "unnamed".into()
    } else {
        out
    }
}

/// Default root for saved attachments: `<data_local_dir>/mail-cli/attachments`.
fn default_attachments_root() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new()
        .ok_or_else(|| Error::Config("cannot determine home directory".into()))?;
    Ok(dirs
        .data_local_dir()
        .join("mail-cli")
        .join("attachments"))
}

/// Build the per-message directory and save attachments to it. Returns
/// `(dir, [{index, filename, mime, size, path}])`.
fn save_attachments_to_dir(
    root: &Path,
    account_name: &str,
    folder: &str,
    uid: &str,
    attachments: &[OwnedAttachment],
) -> Result<(PathBuf, Vec<serde_json::Value>)> {
    let dir = root
        .join(sanitize_component(account_name))
        .join(sanitize_component(folder))
        .join(sanitize_component(uid));
    std::fs::create_dir_all(&dir)?;

    let mut records = Vec::with_capacity(attachments.len());
    for (i, att) in attachments.iter().enumerate() {
        let base = att
            .filename
            .as_deref()
            .map(sanitize_component)
            .unwrap_or_else(|| format!("attachment-{i}"));
        // Prefix with index so multiple attachments with the same filename don't collide.
        let filename = format!("{i:02}_{base}");
        let path = dir.join(&filename);
        std::fs::write(&path, &att.data)?;
        records.push(serde_json::json!({
            "index": i,
            "filename": att.filename,
            "mime_type": att.mime_type,
            "size": att.data.len(),
            "path": path,
        }));
    }
    Ok((dir, records))
}

async fn pull(args: MessagePullArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    use email::search_query::SearchEmailsQuery;
    use email::search_query::filter::SearchEmailsFilterQuery as F;
    use email::search_query::sort::{
        SearchEmailsSorter, SearchEmailsSorterKind, SearchEmailsSorterOrder,
    };

    let account = load_account(global)?;
    tracing::info!(account = %account.name, "pull: loaded account config");
    let imap = account.open_imap().await?;

    tracing::info!(
        account = %account.name,
        folder = %args.folder,
        limit = args.limit,
        "pull: imap connected, running SEARCH"
    );

    // Resolve IMAP credentials from the OS keyring ONCE up-front. Every
    // subsequent async-imap call in this pull shares this struct — critical on
    // macOS where each get_secret() may pop a Keychain authorization prompt.
    let async_creds = crate::backend::async_imap_fetch::ImapCreds::resolve(
        &account.name,
        &account.cfg,
    )
    .await?;

    // Build filter: (NOT Seen) AND (AfterDate ...)
    let mut filter: Option<F> = None;
    if !args.include_read {
        filter = Some(F::Not(Box::new(F::Flag(Flag::Seen))));
    }
    let date_cutoff = match (&args.since, &args.max_age) {
        (Some(s), None) => Some(parse_since_date(s)?),
        (None, Some(m)) => Some(parse_max_age(m)?),
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };
    if let Some(cutoff) = date_cutoff {
        let df = F::AfterDate(cutoff);
        filter = Some(match filter {
            Some(f) => F::And(Box::new(f), Box::new(df)),
            None => df,
        });
    }

    let sort = vec![SearchEmailsSorter::new(
        SearchEmailsSorterKind::Date,
        SearchEmailsSorterOrder::Descending,
    )];

    let opts = ListEnvelopesOptions {
        page: 0,
        page_size: args.limit as usize,
        query: Some(SearchEmailsQuery {
            filter: filter.clone(),
            sort: Some(sort),
        }),
    };

    // Attempt 1: email-lib server-side SEARCH (fast on well-behaved servers).
    // Attempt 2: async-imap SEARCH + batch FETCH (one session for the whole request;
    //            handles servers whose SEARCH breaks imap-client's parser, like 263.net).
    // Attempt 3: client-side paginated envelope scan (last resort).
    //
    // If Attempt 2 succeeds, it comes back with raw bodies already fetched. We stash
    // those in `prefetched_bodies` so the later fetch loop can skip the wasted
    // email-lib peek attempts.
    let mut prefetched_bodies: std::collections::HashMap<String, Vec<u8>> =
        std::collections::HashMap::new();
    let (envelopes, filter_source): (email::envelope::Envelopes, &'static str) =
        match imap.list_envelopes(&args.folder, opts).await {
            Ok(e) => (e, "server-search"),
            Err(email_lib_err) => {
                tracing::warn!(
                    error = %email_lib_err,
                    "server-side SEARCH failed; trying async-imap SEARCH+FETCH shortcut"
                );

                let criteria = build_imap_search_criteria(!args.include_read, date_cutoff);
                match crate::backend::async_imap_fetch::search_and_fetch(
                    &async_creds,
                    &args.folder,
                    &criteria,
                    args.limit as usize,
                )
                .await
                {
                    Ok(msgs) if !msgs.is_empty() => {
                        // Synthesize envelopes from raw MIME + IMAP flags. Stash raw
                        // bodies so we don't refetch them.
                        let mut envs: Vec<email::envelope::Envelope> = Vec::with_capacity(msgs.len());
                        for m in msgs {
                            if let Some(env) = synth_envelope_from_raw(&m.uid, m.is_seen, &m.raw_body) {
                                envs.push(env);
                            }
                            prefetched_bodies.insert(m.uid, m.raw_body);
                        }
                        envs.sort_by(|a, b| b.date.cmp(&a.date));
                        (
                            email::envelope::Envelopes::from_iter(envs),
                            "async-imap-search",
                        )
                    }
                    Ok(_) => {
                        // async-imap search returned zero — either mailbox is empty or
                        // criteria matched nothing. Skip the client-filter fallback in
                        // that case (there's nothing to find).
                        (email::envelope::Envelopes::from_iter(Vec::new()), "async-imap-search")
                    }
                    Err(async_err) => {
                        tracing::warn!(
                            error = %async_err,
                            "async-imap SEARCH failed too; falling back to client-side paginated scan"
                        );
                        let envs = client_side_filter(
                            &imap,
                            &args.folder,
                            args.limit as usize,
                            args.include_read,
                            date_cutoff,
                        )
                        .await?;
                        (envs, "client-filter")
                    }
                }
            }
        };

    tracing::info!(
        source = filter_source,
        n = envelopes.len(),
        "search returned {} envelopes (via {})",
        envelopes.len(),
        filter_source
    );

    // Resolve attachments root once (only used if attachments requested).
    let want_attachments = !args.no_attachments;
    let attach_root: Option<PathBuf> = if want_attachments {
        let root = match args.attachments_dir.clone() {
            Some(p) => p,
            None => default_attachments_root()?,
        };
        Some(root)
    } else {
        None
    };

    let want_body = matches!(args.body_format, PullBodyFormat::Text);
    let want_fetch = want_body || want_attachments;

    // ── Body fetch: batch async-imap for all UIDs that don't already have
    // prefetched bodies from the async-imap-search shortcut.
    //
    // Design change (2026-07-10): the previous implementation did a Pass 1
    // "try email-lib peek per envelope, then Pass 2 batch async-imap for the
    // ones that failed". On broken servers (263.net) EVERY email-lib peek
    // downloads the full body over the wire only to have imap-client's parser
    // discard it — 5 UIDs = 5 wasted body downloads (~120s). We now always use
    // async-imap for the batch body fetch. Cost on well-behaved servers
    // (QQ, Gmail): one extra IMAP connection (~500ms) instead of reusing the
    // existing email-lib session — a good trade for the 120s recovery on 263.
    let uids_needing_bodies: Vec<String> = if want_fetch {
        envelopes
            .iter()
            .filter(|e| !prefetched_bodies.contains_key(&e.id))
            .map(|e| e.id.clone())
            .collect()
    } else {
        Vec::new()
    };
    if !uids_needing_bodies.is_empty() {
        tracing::info!(
            n = uids_needing_bodies.len(),
            "fetching {} message bodies in parallel...",
            uids_needing_bodies.len()
        );
        let started = std::time::Instant::now();
        match crate::backend::async_imap_fetch::fetch_raw_by_uids_parallel(
            &async_creds,
            &args.folder,
            &uids_needing_bodies,
            None, // use default concurrency
        )
        .await
        {
            Ok(map) => {
                let ms = started.elapsed().as_millis();
                tracing::info!(
                    fetched = map.len(),
                    duration_ms = ms as u64,
                    "body fetch done in {} ms",
                    ms
                );
                for (uid, bytes) in map {
                    prefetched_bodies.insert(uid, bytes);
                }
            }
            Err(e) => tracing::warn!(error = %e, "parallel body fetch failed"),
        }
    }

    // Build per-envelope content from prefetched raw bytes (or email-lib peek
    // as last resort — some servers may only respond via that path if
    // async-imap connections are blocked at the network layer).
    let mut per_env: Vec<(email::envelope::Envelope, Option<FetchedMessage>)> =
        Vec::with_capacity(envelopes.len());

    for env in envelopes.iter() {
        let content = if want_fetch {
            if let Some(raw) = prefetched_bodies.get(&env.id) {
                mail_parser::MessageParser::default()
                    .parse(raw)
                    .map(|parsed| {
                        let (b, h, r) = extract_body(&parsed);
                        let attachments = if want_attachments {
                            extract_attachments_owned(&parsed)
                        } else {
                            Vec::new()
                        };
                        FetchedMessage {
                            body_text: b,
                            html_stripped: h,
                            remote_resources: r,
                            source: "async-imap",
                            attachments,
                        }
                    })
            } else {
                // Last resort: original per-envelope email-lib peek.
                try_email_lib_fetch(&imap, &args.folder, &env.id, want_attachments).await
            }
        } else {
            None
        };
        per_env.push((env.clone(), content));
    }

    // ── Emit: build output; save attachments; track successful UIDs ──────
    let mut items: Vec<serde_json::Value> = Vec::with_capacity(per_env.len());
    let mut fetched_ids: Vec<String> = Vec::new();

    for (env, content) in per_env {
        let dto_env = convert::convert_envelope(&env);
        let mut row = serde_json::json!({ "envelope": dto_env });

        if want_fetch {
            match content {
                Some(c) => {
                    if want_body {
                        row["body_text"] = serde_json::Value::String(wrap_untrusted(
                            &env.id,
                            &env.from.addr,
                            &c.body_text,
                        ));
                        row["html_stripped"] = serde_json::Value::Bool(c.html_stripped);
                        row["remote_resources_blocked"] =
                            serde_json::Value::from(c.remote_resources);
                    }
                    row["fetch_source"] = serde_json::Value::String(c.source.to_string());

                    if want_attachments {
                        let root = attach_root.as_ref().unwrap();
                        let (dir, records) = save_attachments_to_dir(
                            root,
                            &account.name,
                            &args.folder,
                            &env.id,
                            &c.attachments,
                        )?;
                        row["attachments_dir"] =
                            serde_json::Value::String(dir.to_string_lossy().into_owned());
                        row["attachments"] = serde_json::Value::Array(records);
                    }
                    fetched_ids.push(env.id.clone());
                }
                None => {
                    // Both email-lib and fallback failed — don't mark this UID as read.
                    if !row.get("fetch_error").is_some() {
                        row["fetch_error"] = serde_json::Value::String(
                            "both email-lib and async-imap fallback returned no body".into(),
                        );
                    }
                }
            }
        } else {
            // envelope-only mode: everything counts as fetched
            fetched_ids.push(env.id.clone());
        }
        items.push(row);
    }

    // ── Batch mark-read the ones we actually got ──────────────────────────
    let mut marked: Vec<String> = Vec::new();
    if !args.peek && !global.read_only && !fetched_ids.is_empty() {
        let flags = Flags::from_iter([Flag::Seen]);
        let batch_id = Id::multiple(email::envelope::MultipleIds::from(fetched_ids.clone()));
        match imap.add_flags(&args.folder, &batch_id, &flags).await {
            Ok(()) => marked = fetched_ids.clone(),
            Err(e) => tracing::warn!(error = %e, "batch mark-read failed"),
        }
    }

    emit(
        &serde_json::json!({
            "pulled": items.len(),
            "marked_read": marked.len(),
            "marked_read_ids": marked,
            "filter_source": filter_source,
            "attachments_saved": attach_root.is_some(),
            "attachments_root": attach_root.map(|p| p.to_string_lossy().into_owned()),
            "filter": {
                "unread_only": !args.include_read,
                "since": args.since,
                "max_age": args.max_age,
                "date_cutoff": date_cutoff.map(|d| d.to_string()),
            },
            "messages": items,
        }),
        fmt,
    )
}

/// Try email-lib's peek path and parse the message on the spot. Returns
/// `Some(FetchedMessage)` on success. On failure (parser miss, empty response,
/// network error), returns `None` — the caller decides whether to try the
/// async-imap fallback.
async fn try_email_lib_fetch(
    imap: &email::backend::Backend<ImapContext>,
    folder: &str,
    uid: &str,
    with_attachments: bool,
) -> Option<FetchedMessage> {
    let single_id: SingleId = uid.to_string().into();
    let id = Id::single(single_id);
    let msgs = imap.peek_messages(folder, &id).await.ok()?;
    let msg = msgs.first()?;
    let parsed = msg.parsed().ok()?;
    let (b, h, r) = extract_body(parsed);
    let attachments = if with_attachments {
        extract_attachments_owned(parsed)
    } else {
        Vec::new()
    };
    Some(FetchedMessage {
        body_text: b,
        html_stripped: h,
        remote_resources: r,
        source: "email-lib",
        attachments,
    })
}

/// Rich fetch result for a single message. Owns all bytes so the parsed message
/// can be dropped before we return.
struct FetchedMessage {
    body_text: String,
    html_stripped: bool,
    remote_resources: u32,
    source: &'static str,
    attachments: Vec<OwnedAttachment>,
}

/// Client-side filter fallback for servers whose IMAP SEARCH implementation
/// breaks imap-client's parser (e.g. 263.net Postfix). Instead of one large
/// SEARCH, we paginate small envelope fetches (newest-first sequence order)
/// and apply the unread / date filter in Rust.
///
/// Optimizations that matter on slow servers:
/// - `BATCH = 30` keeps each individual FETCH small (263.net times out on 200).
/// - Early-stop: if a whole page's newest envelope is older than `date_cutoff`,
///   subsequent pages (older sequence numbers) will be older too — bail immediately.
/// - `MAX_PAGES = 20` caps total scan at 600 envelopes.
async fn client_side_filter(
    imap: &email::backend::Backend<ImapContext>,
    folder: &str,
    limit: usize,
    include_read: bool,
    date_cutoff: Option<chrono::NaiveDate>,
) -> Result<email::envelope::Envelopes> {
    const BATCH: usize = 30;
    const MAX_PAGES: usize = 20;

    let mut matches: Vec<email::envelope::Envelope> = Vec::with_capacity(limit);

    for page in 0..MAX_PAGES {
        let opts = ListEnvelopesOptions {
            page,
            page_size: BATCH,
            query: None,
        };
        let batch = match imap.list_envelopes(folder, opts).await {
            Ok(e) => e,
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("out of bounds") || msg.contains("page range") {
                    break;
                }
                return Err(Error::Transient(format!(
                    "list_envelopes (client-filter page {page}): {e}"
                )));
            }
        };

        let returned = batch.len();
        if returned == 0 {
            break;
        }

        // Early stop: if the newest envelope in this page is already older
        // than the cutoff, subsequent pages (older sequence numbers) are also
        // older. Don't waste RTTs.
        if let Some(cutoff) = date_cutoff {
            let newest = batch.iter().map(|e| e.date).max();
            if let Some(newest) = newest
                && newest.date_naive() < cutoff
            {
                tracing::debug!(page, "all envelopes in page older than cutoff — stopping");
                break;
            }
        }

        for env in batch.iter() {
            if !include_read && env.flags.contains(&Flag::Seen) {
                continue;
            }
            if let Some(cutoff) = date_cutoff
                && env.date.date_naive() < cutoff
            {
                continue;
            }
            matches.push(env.clone());
            if matches.len() >= limit {
                break;
            }
        }

        if matches.len() >= limit {
            break;
        }
        if returned < BATCH {
            break;
        }
    }

    matches.sort_by(|a, b| b.date.cmp(&a.date));
    matches.truncate(limit);
    Ok(email::envelope::Envelopes::from_iter(matches))
}

async fn send(args: MessageSendArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let account = load_account(global)?;
    let body = load_body(&args.body_file)?;

    let raw = build_mime(
        &account,
        &args.to,
        &args.cc,
        &args.bcc,
        &args.subject,
        &body,
        None,
        None,
        &args.attach,
    )?;

    if !args.send {
        return emit(
            &json!({
                "status": "dry-run",
                "from": account.cfg.email,
                "to": args.to,
                "cc": args.cc,
                "bcc": args.bcc,
                "subject": args.subject,
                "body_bytes": body.len(),
                "attachment_count": args.attach.len(),
                "mime_size": raw.len(),
            }),
            fmt,
        );
    }

    if global.read_only {
        return Err(Error::Input(
            "--read-only mode blocks sending; remove --read-only to send".into(),
        ));
    }
    let all_recipients: Vec<String> = args
        .to
        .iter()
        .chain(args.cc.iter())
        .chain(args.bcc.iter())
        .cloned()
        .collect();
    safety::allowlist::check(&all_recipients, &account.cfg.send_allowlist)?;

    let smtp = account.open_smtp().await?;
    smtp.send_message(&raw)
        .await
        .map_err(|e| Error::Transient(format!("send_message: {e}")))?;

    let sent_saved = try_save_sent(&account, &raw, false).await;

    emit(
        &json!({
            "status": "sent",
            "to": args.to,
            "cc": args.cc,
            "bcc": args.bcc,
            "mime_size": raw.len(),
            "sent_saved_id": sent_saved,
        }),
        fmt,
    )
}

async fn reply(args: MessageReplyArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let account = load_account(global)?;

    // We always need the original envelope for headers, so open IMAP now.
    let imap = account.open_imap().await?;
    let single_id: SingleId = args.id.clone().into();
    let orig = imap
        .get_envelope(&args.folder, &single_id)
        .await
        .map_err(|e| Error::Transient(format!("get_envelope: {e}")))?;

    let body = load_body(&args.body_file)?;

    let subject = if orig.subject.to_lowercase().starts_with("re:") {
        orig.subject.clone()
    } else {
        format!("Re: {}", orig.subject)
    };

    let (to, cc) = reply_recipients(&orig, args.reply_all);
    let in_reply_to = orig.message_id.clone();
    let references = orig.message_id.clone();

    let raw = build_mime(
        &account,
        &to,
        &cc,
        &[],
        &subject,
        &body,
        Some(&in_reply_to),
        Some(&references),
        &[],
    )?;

    if !args.send {
        return emit(
            &json!({
                "status": "dry-run",
                "reply_to_id": args.id,
                "from": account.cfg.email,
                "to": to,
                "cc": cc,
                "subject": subject,
                "in_reply_to": in_reply_to,
                "body_bytes": body.len(),
                "mime_size": raw.len(),
            }),
            fmt,
        );
    }

    if global.read_only {
        return Err(Error::Input(
            "--read-only mode blocks sending; remove --read-only to send".into(),
        ));
    }
    let all_recipients: Vec<String> = to.iter().chain(cc.iter()).cloned().collect();
    safety::allowlist::check(&all_recipients, &account.cfg.send_allowlist)?;

    let smtp = account.open_smtp().await?;
    smtp.send_message(&raw)
        .await
        .map_err(|e| Error::Transient(format!("send_message: {e}")))?;

    let sent_saved = try_save_sent_via(&imap, &account, &raw, true).await;

    // Mark original as Answered
    let orig_id = Id::single(single_id);
    let ans = Flags::from_iter([Flag::Answered]);
    if let Err(e) = imap.add_flags(&args.folder, &orig_id, &ans).await {
        warn!(error = %e, "failed to mark original as Answered");
    }

    emit(
        &json!({
            "status": "sent",
            "reply_to_id": args.id,
            "to": to,
            "cc": cc,
            "mime_size": raw.len(),
            "sent_saved_id": sent_saved,
        }),
        fmt,
    )
}

async fn try_save_sent(account: &AccountHandle, raw: &[u8], answered: bool) -> Option<String> {
    let imap = match account.open_imap().await {
        Ok(i) => i,
        Err(e) => {
            warn!(error = %e, "cannot open imap for saving sent copy");
            return None;
        }
    };
    try_save_sent_via(&imap, account, raw, answered).await
}

async fn try_save_sent_via(
    imap: &email::backend::Backend<email::imap::ImapContext>,
    account: &AccountHandle,
    raw: &[u8],
    answered: bool,
) -> Option<String> {
    let sent_folder = account.cfg.sent_folder.as_ref()?;
    let mut flags = Flags::from_iter([Flag::Seen]);
    if answered {
        flags.insert(Flag::Answered);
    }
    match imap.add_message_with_flags(sent_folder, raw, &flags).await {
        Ok(id) => Some(id.to_string()),
        Err(e) => {
            warn!(
                folder = %sent_folder,
                error = %e,
                "failed to save sent copy; send itself succeeded"
            );
            None
        }
    }
}

fn reply_recipients(orig: &EmailEnvelope, reply_all: bool) -> (Vec<String>, Vec<String>) {
    let to = vec![orig.from.addr.clone()];
    let cc = if reply_all {
        // orig.to is a single address in email-lib's Envelope; full-Cc reply
        // (using original To/Cc list) would require parsing the message.
        // For M4 keep it simple.
        vec![orig.to.addr.clone()]
    } else {
        vec![]
    };
    (to, cc)
}

fn parse_flag_input(input: &str) -> Flag {
    // email-lib's Flag::from(&str) already handles both "seen" and "\Seen"-style tokens.
    let trimmed = input.trim_start_matches('\\');
    Flag::from(trimmed)
}

async fn flag(args: MessageFlagArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    if global.read_only {
        return Err(Error::Input("--read-only mode blocks flag changes".into()));
    }
    if args.add.is_empty() && args.remove.is_empty() {
        return Err(Error::Input(
            "at least one of --add or --remove must be given".into(),
        ));
    }

    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let single_id: SingleId = args.id.clone().into();
    let id = Id::single(single_id);

    let added: Vec<Flag> = args.add.iter().map(|s| parse_flag_input(s)).collect();
    let removed: Vec<Flag> = args.remove.iter().map(|s| parse_flag_input(s)).collect();

    if !added.is_empty() {
        let flags = Flags::from_iter(added.clone());
        imap.add_flags(&args.folder, &id, &flags)
            .await
            .map_err(|e| Error::Transient(format!("add_flags: {e}")))?;
    }
    if !removed.is_empty() {
        let flags = Flags::from_iter(removed.clone());
        imap.remove_flags(&args.folder, &id, &flags)
            .await
            .map_err(|e| Error::Transient(format!("remove_flags: {e}")))?;
    }

    let added_names: Vec<String> = added.iter().map(|f| format!("{f:?}")).collect();
    let removed_names: Vec<String> = removed.iter().map(|f| format!("{f:?}")).collect();

    emit(
        &json!({
            "status": "ok",
            "id": args.id,
            "added": added_names,
            "removed": removed_names,
        }),
        fmt,
    )
}

async fn archive(args: MessageArchiveArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    if global.read_only {
        return Err(Error::Input("--read-only mode blocks archive".into()));
    }
    let account = load_account(global)?;
    let target = account.cfg.archive_folder.clone().ok_or_else(|| {
        Error::Config(
            "no archive_folder configured for this account; set it in config.toml".into(),
        )
    })?;

    let imap = account.open_imap().await?;
    let single_id: SingleId = args.id.clone().into();
    let id = Id::single(single_id);

    imap.move_messages(&args.folder, &target, &id)
        .await
        .map_err(|e| Error::Transient(format!("move_messages: {e}")))?;

    emit(
        &json!({
            "status": "archived",
            "id": args.id,
            "from": args.folder,
            "to": target,
        }),
        fmt,
    )
}

async fn delete(args: MessageDeleteArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    if global.read_only {
        return Err(Error::Input("--read-only mode blocks delete".into()));
    }
    // Two-gate: env var + per-call flag.
    safety::delete_gate::check(args.user_explicitly_requested_deletion)?;

    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let single_id: SingleId = args.id.clone().into();
    let id = Id::single(single_id);

    imap.delete_messages(&args.folder, &id)
        .await
        .map_err(|e| Error::Transient(format!("delete_messages: {e}")))?;

    emit(
        &json!({
            "status": "deleted",
            "id": args.id,
            "folder": args.folder,
        }),
        fmt,
    )
}
