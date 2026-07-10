use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub email: String,
    /// Empty by default; send is refused until an address (or wildcard) is added.
    #[serde(default)]
    pub send_allowlist: Vec<String>,
    /// Folder used for `message archive`. If unset, archive is refused with a config error.
    pub archive_folder: Option<String>,
    /// Folder used to save a copy of successfully sent messages. If unset, no copy is saved.
    pub sent_folder: Option<String>,
    pub imap: ImapConfig,
    pub smtp: SmtpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_tls")]
    pub encryption: Encryption,
    pub login: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    #[serde(default = "default_tls")]
    pub encryption: Encryption,
    pub login: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Encryption {
    /// Implicit TLS (typical IMAP 993, SMTPS 465).
    Tls,
    /// Opportunistic STARTTLS (typical IMAP 143, submission 587).
    Starttls,
    /// No transport encryption. Do not use except for local testing.
    None,
}

fn default_tls() -> Encryption {
    Encryption::Tls
}
