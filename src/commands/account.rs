use std::io::{self, IsTerminal, Read};

use serde::Serialize;
use serde_json::json;
use tracing::info;

use crate::cli::{AccountAddArgs, AccountCommand, GlobalArgs};
use crate::config::account::{Encryption, ImapConfig, SmtpConfig};
use crate::config::{AccountConfig, ConfigFile};
use crate::credentials;
use crate::error::{Error, Result};
use crate::output::{OutputFormat, emit};

pub async fn run(cmd: AccountCommand, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        AccountCommand::Add(args) => add(args, global, fmt).await,
        AccountCommand::List => list(global, fmt).await,
        AccountCommand::Remove { name } => remove(&name, global, fmt).await,
    }
}

async fn add(args: AccountAddArgs, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    // Resolve login/email — at least one must be present; missing side falls back to the other.
    let (email, login) = match (args.email.as_deref(), args.login.as_deref()) {
        (Some(e), Some(l)) => (e.to_string(), l.to_string()),
        (Some(e), None) => (e.to_string(), e.to_string()),
        (None, Some(l)) => (l.to_string(), l.to_string()),
        (None, None) => {
            return Err(Error::Input(
                "either --email or --login is required (single value fills both)".into(),
            ));
        }
    };

    let password = resolve_password(&args)?;
    if password.is_empty() {
        return Err(Error::Input("empty password".into()));
    }

    let path = ConfigFile::resolve_path(global.config.as_ref())?;
    let mut cfg = ConfigFile::load(&path)?;

    let existed = cfg.accounts.contains_key(&args.name);
    if existed && !args.force {
        return Err(Error::Input(format!(
            "account '{}' already exists; pass --force to overwrite (this rewrites keyring \
             entries too)",
            args.name
        )));
    }

    let account = AccountConfig {
        email,
        send_allowlist: cfg
            .accounts
            .get(&args.name)
            .map(|prev| prev.send_allowlist.clone())
            .unwrap_or_default(),
        archive_folder: cfg
            .accounts
            .get(&args.name)
            .and_then(|prev| prev.archive_folder.clone()),
        sent_folder: cfg
            .accounts
            .get(&args.name)
            .and_then(|prev| prev.sent_folder.clone()),
        imap: ImapConfig {
            host: args.imap_host,
            port: args.imap_port,
            encryption: Encryption::Tls,
            login: login.clone(),
        },
        smtp: SmtpConfig {
            host: args.smtp_host,
            port: args.smtp_port,
            encryption: Encryption::Tls,
            login,
        },
    };

    credentials::store(&credentials::imap_key(&args.name), &password).await?;
    credentials::store(&credentials::smtp_key(&args.name), &password).await?;

    if cfg.default_account.is_none() {
        cfg.default_account = Some(args.name.clone());
    }
    cfg.accounts.insert(args.name.clone(), account);
    cfg.save(&path)?;

    let status = if existed { "updated" } else { "created" };
    info!(account = %args.name, status, "account saved");
    emit(
        &json!({
            "status": status,
            "account": args.name,
            "config_path": path,
        }),
        fmt,
    )?;
    Ok(())
}

/// Resolve the password from one of three sources (mutually exclusive via clap group).
fn resolve_password(args: &AccountAddArgs) -> Result<String> {
    if let Some(pw) = &args.password {
        return Ok(pw.clone());
    }
    if let Some(var) = &args.password_env {
        return std::env::var(var).map_err(|_| {
            Error::Input(format!(
                "env var `{var}` (from --password-env) is not set or not valid UTF-8"
            ))
        });
    }
    if args.password_stdin {
        let mut stdin = io::stdin();
        if stdin.is_terminal() {
            return Err(Error::Input(
                "--password-stdin was given but stdin is a terminal; pipe the password in, e.g. \
                 `echo \"$PASSWORD\" | mail-cli account add ... --password-stdin`"
                    .into(),
            ));
        }
        let mut buf = String::new();
        stdin.read_to_string(&mut buf)?;
        return Ok(buf.trim_end_matches(&['\n', '\r'][..]).to_string());
    }
    Err(Error::Input(
        "one of --password, --password-env, --password-stdin is required".into(),
    ))
}

async fn list(global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let path = ConfigFile::resolve_path(global.config.as_ref())?;
    let cfg = ConfigFile::load(&path)?;

    #[derive(Serialize)]
    struct Item<'a> {
        name: &'a str,
        email: &'a str,
        default: bool,
        imap: &'a ImapConfig,
        smtp: &'a SmtpConfig,
    }
    let items: Vec<_> = cfg
        .accounts
        .iter()
        .map(|(name, a)| Item {
            name,
            email: &a.email,
            default: cfg.default_account.as_deref() == Some(name.as_str()),
            imap: &a.imap,
            smtp: &a.smtp,
        })
        .collect();

    emit(
        &json!({
            "accounts": items,
            "config_path": path,
            "default_account": cfg.default_account,
        }),
        fmt,
    )?;
    Ok(())
}

async fn remove(name: &str, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let path = ConfigFile::resolve_path(global.config.as_ref())?;
    let mut cfg = ConfigFile::load(&path)?;

    if cfg.accounts.remove(name).is_none() {
        return Err(Error::Input(format!("account '{name}' not found")));
    }
    if cfg.default_account.as_deref() == Some(name) {
        cfg.default_account = cfg.accounts.keys().next().cloned();
    }

    let _ = credentials::delete(&credentials::imap_key(name)).await;
    let _ = credentials::delete(&credentials::smtp_key(name)).await;

    cfg.save(&path)?;
    emit(&json!({ "status": "ok", "removed": name }), fmt)?;
    Ok(())
}
