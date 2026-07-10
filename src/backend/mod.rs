use std::sync::Arc;

use email::account::config::AccountConfig as EmailAccountConfig;
use email::account::config::passwd::PasswordConfig;
use email::backend::{Backend, BackendBuilder};
use email::imap::config::{ImapAuthConfig, ImapConfig as EmailImapConfig};
use email::imap::{ImapContext, ImapContextBuilder};
use email::smtp::config::{SmtpAuthConfig, SmtpConfig as EmailSmtpConfig};
use email::smtp::{SmtpContextBuilder, SmtpContextSync};
use email::tls::Encryption as EmailEncryption;
use secret::Secret;

use crate::config::account::{AccountConfig, Encryption};
use crate::credentials;
use crate::error::{Error, Result};

pub mod async_imap_fetch;
pub mod convert;

/// A handle to a configured account. Cheap to construct; opens no network sockets.
/// Call [`AccountHandle::open_imap`] or [`AccountHandle::open_smtp`] to actually connect.
pub struct AccountHandle {
    pub name: String,
    pub cfg: AccountConfig,
}

impl AccountHandle {
    pub fn new(name: String, cfg: AccountConfig) -> Self {
        Self { name, cfg }
    }

    fn email_account(&self) -> Arc<EmailAccountConfig> {
        Arc::new(EmailAccountConfig {
            name: self.name.clone(),
            email: self.cfg.email.clone(),
            ..Default::default()
        })
    }

    /// Open an IMAP backend with read-side features enabled
    /// (list, peek, get, add-flags, remove-flags, set-flags, move, delete, add-message).
    pub async fn open_imap(&self) -> Result<Backend<ImapContext>> {
        let account = self.email_account();

        let secret = Secret::try_new_keyring_entry(credentials::imap_key(&self.name))
            .map_err(|e| Error::Config(format!("keyring imap entry: {e}")))?;
        let cfg = Arc::new(EmailImapConfig {
            host: self.cfg.imap.host.clone(),
            port: self.cfg.imap.port,
            encryption: Some(map_encryption(self.cfg.imap.encryption)),
            login: self.cfg.imap.login.clone(),
            auth: ImapAuthConfig::Password(PasswordConfig(secret)),
            extensions: None,
            watch: None,
            clients_pool_size: Some(1),
        });
        let ctx = ImapContextBuilder::new(account.clone(), cfg);
        BackendBuilder::new(account, ctx)
            .without_features()
            .with_context_list_folders()
            .with_context_list_envelopes()
            .with_context_get_envelope()
            .with_context_peek_messages()
            .with_context_get_messages()
            .with_context_add_flags()
            .with_context_remove_flags()
            .with_context_set_flags()
            .with_context_move_messages()
            .with_context_delete_messages()
            .with_context_add_message()
            .build()
            .await
            .map_err(|e| Error::Transient(format!("imap backend: {e}")))
    }

    /// Open an SMTP backend with only `send_message` enabled.
    pub async fn open_smtp(&self) -> Result<Backend<SmtpContextSync>> {
        let account = self.email_account();

        let secret = Secret::try_new_keyring_entry(credentials::smtp_key(&self.name))
            .map_err(|e| Error::Config(format!("keyring smtp entry: {e}")))?;
        let cfg = Arc::new(EmailSmtpConfig {
            host: self.cfg.smtp.host.clone(),
            port: self.cfg.smtp.port,
            encryption: Some(map_encryption(self.cfg.smtp.encryption)),
            login: self.cfg.smtp.login.clone(),
            auth: SmtpAuthConfig::Password(PasswordConfig(secret)),
        });
        let ctx = SmtpContextBuilder::new(account.clone(), cfg);
        BackendBuilder::new(account, ctx)
            .without_features()
            .with_context_send_message()
            .build()
            .await
            .map_err(|e| Error::Transient(format!("smtp backend: {e}")))
    }
}

fn map_encryption(e: Encryption) -> EmailEncryption {
    match e {
        Encryption::Tls => EmailEncryption::Tls(Default::default()),
        Encryption::Starttls => EmailEncryption::StartTls(Default::default()),
        Encryption::None => EmailEncryption::None,
    }
}
