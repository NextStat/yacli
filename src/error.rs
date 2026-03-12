use std::process;

use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum YacliError {
    #[error("Account already exists: {0}")]
    AccountExists(String),

    #[error("Account not found: {0}")]
    AccountNotFound(String),

    #[error("No current account configured")]
    CurrentAccountMissing,

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("API error: {0}")]
    Api(String),

    #[error("Auth error: {0}")]
    Auth(String),

    #[error("Output already exists: {0}")]
    OutputExists(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Integrity error: {0}")]
    Integrity(String),
}

impl YacliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::AccountExists(_)
            | Self::AccountNotFound(_)
            | Self::CurrentAccountMissing
            | Self::Validation(_)
            | Self::Auth(_)
            | Self::OutputExists(_)
            | Self::UnsupportedOperation(_) => 1,
            Self::Config(_) | Self::Io(_) | Self::Serialization(_) => 4,
            Self::Network(_) | Self::Api(_) | Self::Integrity(_) => 5,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::AccountExists(_) => "ACCOUNT_EXISTS",
            Self::AccountNotFound(_) => "ACCOUNT_NOT_FOUND",
            Self::CurrentAccountMissing => "CURRENT_ACCOUNT_MISSING",
            Self::Validation(_) => "VALIDATION_ERROR",
            Self::Config(_) => "CONFIG_ERROR",
            Self::Io(_) => "IO_ERROR",
            Self::Serialization(_) => "SERIALIZATION_ERROR",
            Self::Network(_) => "NETWORK_ERROR",
            Self::Api(_) => "API_ERROR",
            Self::Auth(_) => "AUTH_ERROR",
            Self::OutputExists(_) => "OUTPUT_EXISTS",
            Self::UnsupportedOperation(_) => "UNSUPPORTED_OPERATION",
            Self::Integrity(_) => "INTEGRITY_ERROR",
        }
    }

    pub fn as_json(&self) -> serde_json::Value {
        json!({
            "ok": false,
            "code": self.code(),
            "message": self.to_string(),
        })
    }

    pub fn exit(self) -> ! {
        eprintln!("{}", self.as_json());
        process::exit(self.exit_code());
    }
}

impl From<std::io::Error> for YacliError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<toml::de::Error> for YacliError {
    fn from(value: toml::de::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

impl From<toml::ser::Error> for YacliError {
    fn from(value: toml::ser::Error) -> Self {
        Self::Serialization(value.to_string())
    }
}

impl From<reqwest::Error> for YacliError {
    fn from(value: reqwest::Error) -> Self {
        Self::Network(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, YacliError>;
