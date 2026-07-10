use std::io::{self, Write};

use serde::Serialize;

use crate::error::Result;

pub mod envelope;
pub mod error;
pub mod message;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    /// Pretty-printed JSON (human-readable).
    Plain,
    /// Compact JSON (machine-readable).
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        OutputFormat::Plain
    }
}

/// Emit a serializable value to stdout in the requested format.
/// Compact JSON on `Json`, pretty JSON on `Plain`. Trailing newline in both cases.
pub fn emit<T: Serialize>(value: &T, fmt: OutputFormat) -> Result<()> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    match fmt {
        OutputFormat::Json => {
            serde_json::to_writer(&mut lock, value).map_err(io::Error::other)?;
        }
        OutputFormat::Plain => {
            serde_json::to_writer_pretty(&mut lock, value).map_err(io::Error::other)?;
        }
    }
    lock.write_all(b"\n")?;
    Ok(())
}
