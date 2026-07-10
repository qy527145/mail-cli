use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub mod account;
pub use account::AccountConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConfigFile {
    pub default_account: Option<String>,
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountConfig>,
}

impl ConfigFile {
    /// Resolve the config file path, honoring `--config`/`MAIL_CLI_CONFIG` first, then XDG.
    pub fn resolve_path(override_path: Option<&PathBuf>) -> Result<PathBuf> {
        if let Some(p) = override_path {
            return Ok(p.clone());
        }
        let dirs = directories::BaseDirs::new()
            .ok_or_else(|| Error::Config("cannot determine home directory".into()))?;
        Ok(dirs.config_dir().join("mail-cli").join("config.toml"))
    }

    /// Load config from disk. Missing file → empty default (not an error).
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Save config, creating parent dirs as needed.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let s = toml::to_string_pretty(self)?;
        std::fs::write(path, s)?;
        Ok(())
    }

    /// Resolve which account this call refers to: the CLI-provided name,
    /// or the config's `default_account`, or an error.
    pub fn resolve_account_name<'a>(&'a self, requested: Option<&'a str>) -> Result<&'a str> {
        if let Some(name) = requested {
            return Ok(name);
        }
        self.default_account
            .as_deref()
            .ok_or_else(|| Error::Config("no --account and no default_account set".into()))
    }

    pub fn account(&self, name: &str) -> Result<&AccountConfig> {
        self.accounts
            .get(name)
            .ok_or_else(|| Error::Config(format!("account '{name}' not found")))
    }
}
