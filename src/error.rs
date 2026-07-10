use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid input: {0}")]
    Input(String),

    #[error("transient error: {0}")]
    Transient(String),

    #[error("rate limited: {0}")]
    // Part of the public exit-code contract exposed via `agent-info`; will be produced
    // by future protocol backends (Gmail REST, Microsoft Graph) that surface 429s.
    #[allow(dead_code)]
    RateLimit(String),

    #[error("not implemented: {0}")]
    NotImplemented(&'static str),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml deserialize error: {0}")]
    TomlDe(#[from] toml::de::Error),

    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[repr(i32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Ok = 0,
    Transient = 1,
    Config = 2,
    Input = 3,
    RateLimit = 4,
}

impl Error {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Error::Config(_) | Error::TomlDe(_) | Error::TomlSer(_) => ExitCode::Config,
            Error::Input(_) | Error::NotImplemented(_) | Error::Json(_) => ExitCode::Input,
            Error::Transient(_) | Error::Io(_) => ExitCode::Transient,
            Error::RateLimit(_) => ExitCode::RateLimit,
            Error::Other(_) => ExitCode::Transient,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Error::Config(_) => "config",
            Error::Input(_) => "input",
            Error::Transient(_) => "transient",
            Error::RateLimit(_) => "rate_limited",
            Error::NotImplemented(_) => "not_implemented",
            Error::Io(_) => "io",
            Error::Json(_) => "json",
            Error::TomlDe(_) => "toml_parse",
            Error::TomlSer(_) => "toml_serialize",
            Error::Other(_) => "internal",
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
