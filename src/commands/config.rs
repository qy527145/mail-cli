use serde_json::json;

use crate::cli::{ConfigCommand, GlobalArgs};
use crate::config::ConfigFile;
use crate::error::Result;
use crate::output::{OutputFormat, emit};

pub async fn run(cmd: ConfigCommand, global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        ConfigCommand::Show => show(global, fmt).await,
    }
}

async fn show(global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    let path = ConfigFile::resolve_path(global.config.as_ref())?;
    let cfg = ConfigFile::load(&path)?;
    emit(
        &json!({
            "config_path": path,
            "config": cfg,
        }),
        fmt,
    )?;
    Ok(())
}
