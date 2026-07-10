#![recursion_limit = "256"]

mod backend;
mod cli;
mod commands;
mod config;
mod credentials;
mod error;
mod html;
mod output;
mod safety;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::error::{Error, ExitCode};
use crate::output::OutputFormat;
use crate::output::error::{ErrorBody, ErrorOutput};

#[tokio::main]
async fn main() {
    init_tracing();
    init_crypto();
    credentials::init();

    let cli = Cli::parse();
    let fmt = cli.global.effective_format();

    match dispatch(cli).await {
        Ok(()) => std::process::exit(ExitCode::Ok as i32),
        Err(err) => {
            let code = err.exit_code();
            emit_error(&err, fmt, code);
            std::process::exit(code as i32);
        }
    }
}

fn init_tracing() {
    // Default filter: `warn`, but silence imap-codec's "Rectified missing text"
    // noise from RFC-loose servers (263.net / QQ / etc.). Users can override via
    // MAIL_CLI_LOG (e.g. `MAIL_CLI_LOG=imap_codec=warn,email=debug`).
    let filter = EnvFilter::try_from_env("MAIL_CLI_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,imap_codec=error"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// rustls 0.23 requires the process to pick a crypto provider explicitly when
/// multiple providers are theoretically available. `email-lib` sets one up for
/// its own IMAP/SMTP path, but our async-imap fallback constructs its own
/// `ClientConfig` and would panic on TLS init if the process default is unset.
/// Install the ring provider as the process default; ignore Err (already set).
fn init_crypto() {
    let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
}

async fn dispatch(cli: Cli) -> error::Result<()> {
    let Cli { global, command } = cli;
    let fmt = global.effective_format();
    match command {
        Command::AgentInfo => commands::agent_info::run(fmt),
        Command::Account { command } => commands::account::run(command, &global, fmt).await,
        Command::Message { command } => commands::message::run(command, &global, fmt).await,
        Command::Attachment { command } => commands::attachment::run(command, &global, fmt).await,
        Command::Config { command } => commands::config::run(command, &global, fmt).await,
    }
}

fn emit_error(err: &Error, fmt: OutputFormat, code: ExitCode) {
    let body = ErrorOutput {
        error: ErrorBody {
            kind: err.kind().to_string(),
            message: err.to_string(),
            exit_code: code as i32,
        },
    };
    let _ = output::emit(&body, fmt);
}
