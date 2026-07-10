use email::envelope::Id;
use email::message::peek::PeekMessages;
use mail_parser::MimeHeaders;
use serde_json::json;
use tracing::debug;

use crate::backend::AccountHandle;
use crate::cli::{AttachmentCommand, AttachmentDownloadArgs, AttachmentListArgs, GlobalArgs};
use crate::config::ConfigFile;
use crate::error::{Error, Result};
use crate::output::message::AttachmentInfo;
use crate::output::{OutputFormat, emit};

pub async fn run(cmd: AttachmentCommand, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        AttachmentCommand::List(args) => list(args, global, fmt).await,
        AttachmentCommand::Download(args) => download(args, global, fmt).await,
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
