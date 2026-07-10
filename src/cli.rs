use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::output::OutputFormat;

#[derive(Debug, Parser)]
#[command(
    name = "mail-cli",
    version,
    about = "AI agent-friendly email CLI (IMAP + SMTP)",
    long_about = None,
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(subcommand)]
    pub command: Command,
}

/// Global arguments accepted on every subcommand.
#[derive(Debug, Clone, clap::Args)]
pub struct GlobalArgs {
    /// Shorthand for --output json.
    #[arg(long, global = true)]
    pub json: bool,

    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Plain)]
    pub output: OutputFormat,

    /// Path to config file (default: ~/.config/mail-cli/config.toml).
    #[arg(long, global = true, env = "MAIL_CLI_CONFIG")]
    pub config: Option<PathBuf>,

    /// Account to operate on (must be one already added via `account add`).
    #[arg(long, global = true, env = "MAIL_CLI_ACCOUNT")]
    pub account: Option<String>,

    /// Block all mutating operations (send, flag, archive, delete).
    #[arg(long, global = true, env = "MAIL_CLI_READ_ONLY")]
    pub read_only: bool,
}

impl GlobalArgs {
    pub fn effective_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.output
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print a JSON manifest of every command, flag, exit code, and output format.
    AgentInfo,
    /// Account management.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Message operations.
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Attachment operations.
    Attachment {
        #[command(subcommand)]
        command: AttachmentCommand,
    },
    /// Configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Add a new account. Password read from stdin (--password-stdin) into the OS keyring.
    Add(AccountAddArgs),
    /// List configured accounts (no passwords).
    List,
    /// Remove an account and its keyring entries.
    Remove {
        #[arg(long)]
        name: String,
    },
    /// Manage the send_allowlist for an account. Send is blocked to any recipient
    /// not matched by this list — an intentional safety gate against
    /// prompt-injection-driven exfiltration.
    Allowlist {
        #[command(subcommand)]
        command: AllowlistCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum AllowlistCommand {
    /// Add addresses (exact like `alice@x.com` or wildcard like `*@x.com`).
    /// Idempotent: duplicates are ignored.
    Add(AllowlistMutateArgs),
    /// Remove addresses. Missing entries are silently ignored.
    Remove(AllowlistMutateArgs),
    /// Replace the entire allowlist with these addresses.
    Set(AllowlistMutateArgs),
    /// Clear the allowlist. After this, no `--send` will succeed until you
    /// add an address back.
    Clear {
        #[arg(long)]
        name: String,
    },
    /// Show the current allowlist.
    Show {
        #[arg(long)]
        name: String,
    },
}

#[derive(Debug, clap::Args)]
pub struct AllowlistMutateArgs {
    #[arg(long)]
    pub name: String,
    /// One or more addresses. Supports comma-separated (`--send-allow a,b`)
    /// and repeated flags. Wildcards: `*@domain.com` matches any local part.
    #[arg(value_name = "ADDR", required = true, num_args = 1.., value_delimiter = ',')]
    pub addresses: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct AccountAddArgs {
    #[arg(long)]
    pub name: String,
    /// Display email. Falls back to `--login` if omitted.
    #[arg(long)]
    pub email: Option<String>,
    #[arg(long)]
    pub imap_host: String,
    #[arg(long, default_value_t = 993)]
    pub imap_port: u16,
    #[arg(long)]
    pub smtp_host: String,
    #[arg(long, default_value_t = 465)]
    pub smtp_port: u16,
    /// IMAP/SMTP login. Falls back to `--email` if omitted.
    #[arg(long)]
    pub login: Option<String>,

    /// Overwrite an existing account with the same name. Without this, add refuses to touch it.
    #[arg(long)]
    pub force: bool,

    // ── Password sources — exactly one required ─────────────────────────
    /// Password as a direct value. WARNING: visible in shell history and `ps aux`.
    /// Prefer --password-env or --password-stdin for anything you actually care about.
    #[arg(long, group = "password_source")]
    pub password: Option<String>,
    /// Name of an env var holding the password (e.g. --password-env MAIL_PW).
    #[arg(long, group = "password_source", value_name = "ENV_VAR")]
    pub password_env: Option<String>,
    /// Read password from stdin. Refuses to run if stdin is a terminal.
    #[arg(long, group = "password_source")]
    pub password_stdin: bool,

    /// Seed the send_allowlist at account creation. Repeatable and comma-separated.
    /// Example: `--send-allow boss@x.com --send-allow "*@team.com"` or `--send-allow a,b,c`.
    /// With --force this REPLACES the existing allowlist (only when explicitly passed;
    /// omitting the flag preserves the existing list).
    #[arg(long = "send-allow", value_name = "ADDR", num_args = 1.., value_delimiter = ',', action = clap::ArgAction::Append)]
    pub send_allow: Vec<String>,

    /// Make this the default account for all future commands (used when
    /// `--account` and env `MAIL_CLI_ACCOUNT` are both unset). Without this,
    /// the first account added becomes default; subsequent adds preserve the
    /// existing default.
    #[arg(long = "default", short = 'p')]
    pub set_default: bool,
}

#[derive(Debug, Subcommand)]
pub enum MessageCommand {
    List(MessageListArgs),
    Read(MessageReadArgs),
    Pull(MessagePullArgs),
    Send(MessageSendArgs),
    Reply(MessageReplyArgs),
    Flag(MessageFlagArgs),
    Archive(MessageArchiveArgs),
    Delete(MessageDeleteArgs),
}

#[derive(Debug, clap::Args)]
pub struct MessageListArgs {
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    #[arg(long, default_value_t = 0)]
    pub page: u32,
    #[arg(long)]
    pub unread: bool,
    /// ISO-8601 date; only envelopes on or after this date are listed.
    #[arg(long)]
    pub since: Option<String>,
}

#[derive(Debug, clap::Args)]
pub struct MessageReadArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    #[arg(long, value_enum, default_value_t = MessageFormat::Text)]
    pub format: MessageFormat,
    /// Set the \Seen flag. Default is a non-mutating peek.
    #[arg(long)]
    pub mark_read: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum MessageFormat {
    /// Plain text (HTML converted via html2text).
    Text,
    /// Raw RFC-822 bytes (base64-encoded in JSON output).
    Raw,
}

/// Batch pull of unread mail — the canonical agent inbox-poll operation.
///
/// Combines filter + body fetch + optional mark-as-read in one call so agents
/// don't have to compose 3 separate commands (and so the "mark read" happens
/// only for messages that were successfully fetched — no message ever gets
/// lost silently).
#[derive(Debug, clap::Args)]
pub struct MessagePullArgs {
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    /// Maximum number of envelopes to return (newest first).
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    /// Only include messages with `Date:` on or after this date (YYYY-MM-DD).
    /// Mutually exclusive with --max-age.
    #[arg(long, conflicts_with = "max_age")]
    pub since: Option<String>,
    /// Only include messages received in the last N (m|h|d), e.g. `30m`, `2h`, `7d`.
    /// Mutually exclusive with --since.
    #[arg(long, conflicts_with = "since", value_name = "DURATION")]
    pub max_age: Option<String>,
    /// Also include already-read messages (default: unread only).
    #[arg(long)]
    pub include_read: bool,
    /// Do not mark any messages as read after fetch (default: mark successful ones).
    #[arg(long)]
    pub peek: bool,
    /// Body format returned in each message. `none` returns envelope-only (cheapest for agents).
    #[arg(long, value_enum, default_value_t = PullBodyFormat::Text)]
    pub body_format: PullBodyFormat,
    /// Skip attachment download. By default `pull` DOES fetch and save attachments
    /// (one directory per message under `attachments_dir`) because that's what
    /// most agent workflows want. Pass this to skip attachments (saves disk/RTT).
    #[arg(long)]
    pub no_attachments: bool,
    /// Root directory for saved attachments. Defaults to
    /// `<data_local_dir>/mail-cli/attachments`. Each message gets its own
    /// subdirectory `<root>/<account>/<folder>/<uid>/`.
    #[arg(long, value_name = "PATH")]
    pub attachments_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PullBodyFormat {
    /// Return body (plain text, HTML→text via html2text), with `<UNTRUSTED_EMAIL_BODY>` wrapper.
    Text,
    /// Return envelope only (no body fetch — no async-imap fallback either).
    None,
}

#[derive(Debug, clap::Args)]
pub struct MessageSendArgs {
    #[arg(long, value_delimiter = ',', required = true)]
    pub to: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub cc: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub bcc: Vec<String>,
    #[arg(long)]
    pub subject: String,
    /// Body text passed directly on the command line. Mutually exclusive with
    /// --body-file. Best for short bodies; visible in shell history.
    #[arg(long, group = "body_source")]
    pub body: Option<String>,
    /// Body file path, or `-` to read from stdin. Mutually exclusive with --body.
    #[arg(long, group = "body_source")]
    pub body_file: Option<String>,
    #[arg(long)]
    pub attach: Vec<PathBuf>,
    /// Do not actually send; return the assembled MIME (default behavior).
    #[arg(long, group = "send_mode")]
    pub dry_run: bool,
    /// Actually send. Recipients must be in the account's send_allowlist.
    #[arg(long, group = "send_mode")]
    pub send: bool,
}

#[derive(Debug, clap::Args)]
pub struct MessageReplyArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    /// Body text passed directly on the command line. Mutually exclusive with --body-file.
    #[arg(long, group = "reply_body_source")]
    pub body: Option<String>,
    /// Body file path, or `-` to read from stdin. Mutually exclusive with --body.
    #[arg(long, group = "reply_body_source")]
    pub body_file: Option<String>,
    #[arg(long)]
    pub reply_all: bool,
    #[arg(long, group = "send_mode")]
    pub dry_run: bool,
    #[arg(long, group = "send_mode")]
    pub send: bool,
}

#[derive(Debug, clap::Args)]
pub struct MessageFlagArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    /// Flags to add. Use IANA names ("seen", "flagged") or raw (`\Seen`).
    #[arg(long)]
    pub add: Vec<String>,
    #[arg(long)]
    pub remove: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub struct MessageArchiveArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
}

#[derive(Debug, clap::Args)]
pub struct MessageDeleteArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    /// Required alongside env MAIL_CLI_DELETE_ENABLED=true. Both gates must pass.
    #[arg(long)]
    pub user_explicitly_requested_deletion: bool,
}

#[derive(Debug, Subcommand)]
pub enum AttachmentCommand {
    List(AttachmentListArgs),
    Download(AttachmentDownloadArgs),
    /// Delete previously saved attachment directories (from `message pull --attachments`).
    /// Requires at least one scoping flag; there is no accidental "clear all".
    Clear(AttachmentClearArgs),
}

#[derive(Debug, clap::Args)]
pub struct AttachmentListArgs {
    #[arg(long)]
    pub message_id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
}

#[derive(Debug, clap::Args)]
pub struct AttachmentDownloadArgs {
    #[arg(long)]
    pub message_id: String,
    #[arg(long, default_value = "INBOX")]
    pub folder: String,
    #[arg(long)]
    pub index: u32,
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct AttachmentClearArgs {
    /// Delete every message directory under the attachment root. Refuses to run without this.
    #[arg(long, conflicts_with_all = ["older_than", "account_scope", "folder_scope"])]
    pub all: bool,
    /// Delete message dirs whose mtime is older than DURATION (e.g. `7d`, `24h`, `30m`).
    #[arg(long, value_name = "DURATION")]
    pub older_than: Option<String>,
    /// Scope: only touch dirs under `<root>/<account>/`.
    #[arg(long = "account-scope", value_name = "NAME")]
    pub account_scope: Option<String>,
    /// Scope: only touch dirs under `<root>/<account>/<folder>/`. Requires --account-scope.
    #[arg(long = "folder-scope", value_name = "NAME", requires = "account_scope")]
    pub folder_scope: Option<String>,
    /// Override the attachment root (default: `<data_local_dir>/mail-cli/attachments`).
    #[arg(long, value_name = "PATH")]
    pub attachments_dir: Option<PathBuf>,
    /// Preview what would be deleted without actually removing anything.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print current config (without secrets).
    Show,
}
