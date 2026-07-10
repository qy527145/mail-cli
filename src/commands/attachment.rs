use std::path::{Path, PathBuf};
use std::time::SystemTime;

use email::envelope::Id;
use email::message::peek::PeekMessages;
use mail_parser::MimeHeaders;
use serde_json::json;
use tracing::debug;

use crate::backend::AccountHandle;
use crate::cli::{
    AttachmentClearArgs, AttachmentCommand, AttachmentDownloadArgs, AttachmentListArgs, GlobalArgs,
};
use crate::config::ConfigFile;
use crate::error::{Error, Result};
use crate::output::message::AttachmentInfo;
use crate::output::{OutputFormat, emit};

pub async fn run(cmd: AttachmentCommand, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        AttachmentCommand::List(args) => list(args, global, fmt).await,
        AttachmentCommand::Download(args) => download(args, global, fmt).await,
        AttachmentCommand::Clear(args) => clear(args, global, fmt).await,
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

async fn list(args: AttachmentListArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let id = Id::single(args.message_id.clone());

    let msgs = imap
        .peek_messages(&args.folder, &id)
        .await
        .map_err(|e| Error::Transient(format!("peek_messages: {e}")))?;
    let msg = msgs
        .first()
        .ok_or_else(|| Error::Input(format!("message '{}' not found", args.message_id)))?;
    let parsed = msg
        .parsed()
        .map_err(|e| Error::Transient(format!("parse message: {e}")))?;

    let attachments: Vec<AttachmentInfo> = parsed
        .attachments()
        .enumerate()
        .map(|(idx, part)| AttachmentInfo {
            index: idx as u32,
            filename: part.attachment_name().map(str::to_string),
            mime_type: part
                .content_type()
                .map(|ct| match ct.c_subtype.as_ref() {
                    Some(sub) => format!("{}/{}", ct.c_type, sub),
                    None => ct.c_type.to_string(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_string()),
            size: part.contents().len() as u64,
        })
        .collect();

    emit(
        &json!({
            "message_id": args.message_id,
            "attachments": attachments,
        }),
        fmt,
    )
}

async fn download(
    args: AttachmentDownloadArgs,
    global: &GlobalArgs,
    fmt: OutputFormat,
) -> Result<()> {
    let account = load_account(global)?;
    let imap = account.open_imap().await?;
    let id = Id::single(args.message_id.clone());

    let msgs = imap
        .peek_messages(&args.folder, &id)
        .await
        .map_err(|e| Error::Transient(format!("peek_messages: {e}")))?;
    let msg = msgs
        .first()
        .ok_or_else(|| Error::Input(format!("message '{}' not found", args.message_id)))?;
    let parsed = msg
        .parsed()
        .map_err(|e| Error::Transient(format!("parse message: {e}")))?;

    let part = parsed.attachment(args.index as usize).ok_or_else(|| {
        Error::Input(format!(
            "attachment index {} not found (has {})",
            args.index,
            parsed.attachment_count()
        ))
    })?;

    let bytes = part.contents();
    debug!(bytes = bytes.len(), path = %args.output.display(), "writing attachment");
    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.output, bytes)?;

    emit(
        &json!({
            "status": "ok",
            "message_id": args.message_id,
            "index": args.index,
            "output": args.output,
            "size": bytes.len(),
            "filename": part.attachment_name(),
        }),
        fmt,
    )
}

fn default_attachments_root() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new()
        .ok_or_else(|| Error::Config("cannot determine home directory".into()))?;
    Ok(dirs.data_local_dir().join("mail-cli").join("attachments"))
}

fn parse_max_age(s: &str) -> Result<std::time::Duration> {
    let (num_part, unit) = s.split_at(
        s.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| Error::Input(format!("--older-than missing unit (m/h/d) in {s:?}")))?,
    );
    let n: u64 = num_part
        .parse()
        .map_err(|_| Error::Input(format!("--older-than not a number: {num_part:?}")))?;
    let secs = match unit {
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => {
            return Err(Error::Input(format!(
                "--older-than unit must be m|h|d (got {unit:?})"
            )));
        }
    };
    Ok(std::time::Duration::from_secs(secs))
}

/// Total size of every regular file in `dir` (best-effort, non-recursive per level).
fn dir_size_bytes(dir: &Path) -> u64 {
    let mut total = 0u64;
    if let Ok(iter) = std::fs::read_dir(dir) {
        for entry in iter.flatten() {
            let path = entry.path();
            if let Ok(md) = std::fs::metadata(&path) {
                if md.is_file() {
                    total += md.len();
                } else if md.is_dir() {
                    total += dir_size_bytes(&path);
                }
            }
        }
    }
    total
}

async fn clear(args: AttachmentClearArgs, _global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    // Require at least one scoping flag — no accidental "wipe everything".
    if !args.all
        && args.older_than.is_none()
        && args.account_scope.is_none()
        && args.folder_scope.is_none()
    {
        return Err(Error::Input(
            "at least one of --all, --older-than, --account-scope must be given \
             (defensive default; there is no implicit \"clear everything\")"
                .into(),
        ));
    }

    let root = match args.attachments_dir {
        Some(p) => p,
        None => default_attachments_root()?,
    };
    if !root.exists() {
        return emit(
            &json!({
                "status": "ok",
                "root": root,
                "deleted_dirs": 0,
                "freed_bytes": 0,
                "dry_run": args.dry_run,
                "note": "attachment root does not exist yet",
            }),
            fmt,
        );
    }

    // Determine which subtree to walk.
    let walk_start = match (&args.account_scope, &args.folder_scope) {
        (Some(acc), Some(f)) => root.join(sanitize_component(acc)).join(sanitize_component(f)),
        (Some(acc), None) => root.join(sanitize_component(acc)),
        (None, None) => root.clone(),
        (None, Some(_)) => unreachable!("clap requires account-scope for folder-scope"),
    };
    if !walk_start.exists() {
        return emit(
            &json!({
                "status": "ok",
                "root": root,
                "walk_start": walk_start,
                "deleted_dirs": 0,
                "freed_bytes": 0,
                "dry_run": args.dry_run,
                "note": "scope directory does not exist",
            }),
            fmt,
        );
    }

    let max_age = args
        .older_than
        .as_deref()
        .map(parse_max_age)
        .transpose()?;
    let now = SystemTime::now();

    // Collect message-level directories (deepest leaf level).
    let mut candidates: Vec<PathBuf> = Vec::new();
    collect_message_dirs(&walk_start, &mut candidates);

    // Filter by age if requested.
    let selected: Vec<PathBuf> = if let Some(age) = max_age {
        candidates
            .into_iter()
            .filter(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .map(|mtime| now.duration_since(mtime).map(|d| d >= age).unwrap_or(false))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        candidates
    };

    let mut deleted = 0u64;
    let mut freed = 0u64;
    let mut examples: Vec<PathBuf> = Vec::new();
    for dir in &selected {
        let size = dir_size_bytes(dir);
        if !args.dry_run {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                tracing::warn!(dir = %dir.display(), error = %e, "remove_dir_all failed; skipping");
                continue;
            }
        }
        deleted += 1;
        freed += size;
        if examples.len() < 10 {
            examples.push(dir.clone());
        }
    }

    // Best-effort: also prune empty parent directories that we may have emptied.
    if !args.dry_run {
        prune_empty_parents(&walk_start, &root);
    }

    emit(
        &json!({
            "status": "ok",
            "root": root,
            "walk_start": walk_start,
            "dry_run": args.dry_run,
            "deleted_dirs": deleted,
            "freed_bytes": freed,
            "examples": examples,
        }),
        fmt,
    )
}

/// Walk to the "message-level" leaf directories. Our attachment layout is
/// `<root>/<account>/<folder>/<uid>/`, so we look for dirs whose children are
/// all files (no more subdirs).
fn collect_message_dirs(start: &Path, out: &mut Vec<PathBuf>) {
    let Ok(iter) = std::fs::read_dir(start) else {
        return;
    };
    let entries: Vec<_> = iter.flatten().collect();
    let mut has_subdir = false;
    for e in &entries {
        if e.path().is_dir() {
            has_subdir = true;
            break;
        }
    }
    if !has_subdir {
        // Leaf: this dir contains only files (or is empty).
        out.push(start.to_path_buf());
        return;
    }
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            collect_message_dirs(&p, out);
        }
    }
}

/// Remove empty directories walking upward from `start` toward `root`, but never remove `root` itself.
fn prune_empty_parents(start: &Path, root: &Path) {
    let mut cur = start.to_path_buf();
    while cur != *root && cur.starts_with(root) {
        if let Ok(mut iter) = std::fs::read_dir(&cur) {
            if iter.next().is_none() {
                let _ = std::fs::remove_dir(&cur);
            } else {
                break;
            }
        } else {
            break;
        }
        if let Some(parent) = cur.parent() {
            cur = parent.to_path_buf();
        } else {
            break;
        }
    }
}

/// Same sanitizer used when saving; keeps clear/save symmetric.
fn sanitize_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '\0' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    if out.is_empty() { "unnamed".into() } else { out }
}
