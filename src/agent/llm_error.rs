use thiserror::Error;

use crate::agent::providers::ToolCall;

#[derive(Error, Debug)]
pub enum LlmError {
    #[error("Approval required for tool '{0}': {1:?}")]
    ApprovalRequired(String, ToolCall),

    #[error("API request failed: {0}")]
    ApiRequestFailed(#[from] reqwest::Error),

    #[error("Provider error {status}: {message}")]
    ProviderError {
        status: u16,
        message: String,
    },

    #[error("Rate limited (429): {0}")]
    RateLimit(String),

    #[error("Context window exceeded: {0}")]
    ContextWindowExceeded(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("Unknown error: {0}")]
    Unknown(#[from] anyhow::Error),
}

impl LlmError {
    pub fn is_rate_limit(&self) -> bool {
        match self {
            LlmError::RateLimit(_) => true,
            LlmError::ProviderError { status, .. } => *status == 429,
            LlmError::ApiRequestFailed(e) => e.status().map(|s| s.as_u16() == 429).unwrap_or(false),
            _ => false,
        }
    }

    pub fn status_code(&self) -> Option<u16> {
        match self {
            LlmError::ProviderError { status, .. } => Some(*status),
            LlmError::ApiRequestFailed(e) => e.status().map(|s| s.as_u16()),
            LlmError::RateLimit(_) => Some(429),
            _ => None,
        }
    }
}
