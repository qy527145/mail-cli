use keyring::KeyringEntry;

use crate::error::{Error, Result};

pub const KEYRING_SERVICE: &str = "mail-cli";

/// Set the global keyring service name. Call once at process start.
pub fn init() {
    keyring::set_global_service_name(KEYRING_SERVICE);
}

pub fn imap_key(account_name: &str) -> String {
    format!("{account_name}:imap-passwd")
}

pub fn smtp_key(account_name: &str) -> String {
    format!("{account_name}:smtp-passwd")
}

/// Store a password in the OS keyring under the given key.
pub async fn store(key: &str, password: &str) -> Result<()> {
    let entry =
        KeyringEntry::try_new(key).map_err(|e| Error::Config(format!("keyring: {e:#}")))?;
    entry
        .set_secret(password)
        .await
        .map_err(|e| {
            // Walk the source chain for a useful native error message.
            let mut msg = format!("keyring set `{key}`: {e}");
            let mut src = std::error::Error::source(&e);
            while let Some(s) = src {
                msg.push_str(&format!(" -> {s}"));
                src = s.source();
            }
            Error::Config(msg)
        })?;
    Ok(())
}

/// Load a password from the OS keyring.
pub async fn load(key: &str) -> Result<String> {
    let entry =
        KeyringEntry::try_new(key).map_err(|e| Error::Config(format!("keyring: {e:#}")))?;
    entry.get_secret().await.map_err(|e| {
        let mut msg = format!("keyring get `{key}`: {e}");
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(&format!(" -> {s}"));
            src = s.source();
        }
        Error::Config(msg)
    })
}

/// Delete a keyring entry. Missing entries are silently ignored.
pub async fn delete(key: &str) -> Result<()> {
    let entry = match KeyringEntry::try_new(key) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    let _ = entry.delete_secret().await;
    Ok(())
}
