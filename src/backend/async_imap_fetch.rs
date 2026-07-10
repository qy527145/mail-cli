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

use std::sync::Arc;

use async_imap::Client;
use futures::TryStreamExt;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing::debug;

use crate::config::account::{AccountConfig, Encryption};
use crate::credentials;
use crate::error::{Error, Result};

/// Connect to the account's IMAP server, log in with the keyring-stored password,
/// select `folder`, `UID FETCH <uid> BODY.PEEK[]`, and return the raw RFC-822 bytes.
///
/// This bypasses `email-lib` entirely for the fetch — it is only used as a
/// fallback when the primary path failed with an empty result. Passwords are
/// still read from the keyring under the same key.
pub async fn fetch_raw_by_uid(
    account_name: &str,
    cfg: &AccountConfig,
    folder: &str,
    uid: &str,
) -> Result<Vec<u8>> {
    let host = cfg.imap.host.clone();
    let port = cfg.imap.port;
    let login = cfg.imap.login.clone();
    let password = credentials::load(&credentials::imap_key(account_name)).await?;

    debug!(host = %host, port, login = %login, folder, uid, "async-imap fallback fetch");

    match cfg.imap.encryption {
        Encryption::Tls => fetch_tls(&host, port, &login, &password, folder, uid).await,
        Encryption::Starttls => Err(Error::NotImplemented(
            "STARTTLS in async-imap fallback (v0.1: use tls only)",
        )),
        Encryption::None => Err(Error::Config(
            "plaintext IMAP not supported in fallback path".into(),
        )),
    }
}

async fn fetch_tls(
    host: &str,
    port: u16,
    login: &str,
    password: &str,
    folder: &str,
    uid: &str,
) -> Result<Vec<u8>> {
    // Build a rustls client config with Mozilla roots.
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));

    // TCP → TLS
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|e| Error::Transient(format!("tcp connect {host}:{port}: {e}")))?;
    let server_name = ServerName::try_from(host.to_string())
        .map_err(|e| Error::Config(format!("invalid hostname `{host}`: {e}")))?;
    let tls = connector
        .connect(server_name, tcp)
        .await
        .map_err(|e| Error::Transient(format!("tls handshake: {e}")))?;

    // async-imap with runtime-tokio consumes tokio::io traits directly. No compat layer needed.
    let mut client = Client::new(tls);

    // read greeting
    let _greeting = client
        .read_response()
        .await
        .map_err(|e| Error::Transient(format!("imap greeting: {e}")))?
        .ok_or_else(|| Error::Transient("imap greeting: EOF".into()))?;

    // LOGIN
    let mut session = client
        .login(login, password)
        .await
        .map_err(|(e, _client)| Error::Config(format!("imap login: {e}")))?;

    // SELECT
    session
        .select(folder)
        .await
        .map_err(|e| Error::Transient(format!("select `{folder}`: {e}")))?;

    // UID FETCH <uid> BODY.PEEK[]
    // Streams a series of Fetch responses; for a single UID we take the first non-empty body.
    let mut fetches = session
        .uid_fetch(uid, "BODY.PEEK[]")
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch: {e}")))?;

    let mut raw: Option<Vec<u8>> = None;
    while let Some(fetch) = fetches
        .try_next()
        .await
        .map_err(|e| Error::Transient(format!("uid_fetch stream: {e}")))?
    {
        if let Some(body) = fetch.body() {
            raw = Some(body.to_vec());
            break;
        }
    }
    // Drop the stream borrow before calling logout on the session again.
    drop(fetches);

    // Best-effort logout; failures here are not fatal for the caller.
    let _ = session.logout().await;

    raw.ok_or_else(|| {
        Error::Transient(format!(
            "async-imap fallback: no body returned for UID {uid} in {folder}"
        ))
    })
}
