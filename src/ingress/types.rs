use crate::agent::ImageAttachment;
use std::fmt;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    /// Authenticated user input (e.g., from Telegram with verified ID).
    /// Full capabilities, including file system and network.
    OwnerCommand,

    /// Signals from internal daemons or user-written scripts.
    /// Scoped capabilities (defined by JobSpec).
    TrustedEvent,

    /// External data (Webhooks, Forwarded Messages, Emails).
    /// No capabilities. Text extraction/sanitization only.
    UntrustedEvent,
}

#[derive(Debug, Clone)]
pub struct IngressMessage {
    pub id: Uuid,
    pub source: String,
    pub payload: String, // could be JSON, or just text
    pub trust: TrustLevel,
    pub timestamp: u64,
    pub images: Vec<ImageAttachment>,
}

impl IngressMessage {
    pub fn new(source: String, payload: String, trust: TrustLevel) -> Self {
        Self {
            id: Uuid::new_v4(),
            source,
            payload,
            trust,
            timestamp: chrono::Utc::now().timestamp() as u64,
            images: Vec::new(),
        }
    }

    pub fn with_images(mut self, images: Vec<ImageAttachment>) -> Self {
        self.images = images;
        self
    }
}

impl fmt::Display for IngressMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redact payload if it's too long or if we want to be careful (trace only)
        // For Display, we just show metadata.
        write!(
            f,
            "[{}] {} from {} (Trust: {:?})",
            self.timestamp, self.id, self.source, self.trust
        )
    }
}
