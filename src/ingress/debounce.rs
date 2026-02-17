use super::types::IngressMessage;
use crate::config::IngressDebounceConfig;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct DebounceSession {
    buffer: Vec<IngressMessage>,
    last_update: Instant,
}

impl DebounceSession {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            last_update: Instant::now(),
        }
    }

    /// Total characters in buffered payloads
    fn char_count(&self) -> usize {
        self.buffer.iter().map(|m| m.payload.len()).sum()
    }
}

#[derive(Debug)]
pub struct DebounceManager {
    sessions: HashMap<String, DebounceSession>,
    config: IngressDebounceConfig,
}

impl DebounceManager {
    /// Create a new DebounceManager with the given configuration.
    pub fn new(config: IngressDebounceConfig) -> Self {
        Self {
            sessions: HashMap::new(),
            config,
        }
    }

    /// Ingest a new message for the given source.
    /// The message is buffered; if size or character limits are exceeded,
    /// the session is immediately marked as ready for flushing.
    pub fn ingest(&mut self, msg: IngressMessage) {
        let source = msg.source.clone();
        let session = self
            .sessions
            .entry(source)
            .or_insert_with(DebounceSession::new);
        session.buffer.push(msg);
        session.last_update = Instant::now();

        // Check limits: if exceeded, mark session as ready by setting last_update far enough in the past.
        if session.buffer.len() > self.config.max_debounce_messages
            || session.char_count() > self.config.max_debounce_chars
        {
            let debounce_dur = Duration::from_secs(self.config.debounce_seconds);
            session.last_update = Instant::now() - debounce_dur - Duration::from_secs(1);
        }
    }

    /// Flush all sessions that have been quiet for at least `debounce_seconds`.
    /// Returns a vector of combined IngressMessages (one per source).
    pub fn flush_ready(&mut self, now: Instant) -> Vec<IngressMessage> {
        let debounce_dur = Duration::from_secs(self.config.debounce_seconds);
        let mut ready = Vec::new();
        let mut to_remove = Vec::new();

        for (source, session) in self.sessions.iter_mut() {
            if now.duration_since(session.last_update) >= debounce_dur {
                if !session.buffer.is_empty() {
                    let combined = combine_session(session, source);
                    ready.push(combined);
                    to_remove.push(source.clone());
                }
            }
        }

        // Remove flushed sessions
        for source in to_remove {
            self.sessions.remove(&source);
        }

        ready
    }

    /// Flush all remaining sessions (e.g., on shutdown). Returns combined messages for all sources.
    pub fn flush_all(&mut self) -> Vec<IngressMessage> {
        let mut all = Vec::new();
        for (source, session) in self.sessions.drain() {
            if !session.buffer.is_empty() {
                let combined = combine_session(&session, &source);
                all.push(combined);
            }
        }
        all
    }
}

/// Combine all messages in a session into a single IngressMessage.
/// - source: the source string
/// - id: new UUID
/// - timestamp: earliest timestamp among buffered messages
/// - trust: from first message (assumed uniform)
/// - payload: concatenated payloads separated by "\n\n"
/// - images: concatenated images in order
fn combine_session(session: &DebounceSession, source: &str) -> IngressMessage {
    if session.buffer.is_empty() {
        panic!("combine_session called on empty session");
    }

    // Determine earliest timestamp
    let earliest_ts = session
        .buffer
        .iter()
        .map(|m| m.timestamp)
        .min()
        .unwrap_or(0);

    // Trust should be consistent; use first
    let trust = session.buffer[0].trust;

    // Concatenate payloads
    let mut combined_payload = String::new();
    for (i, msg) in session.buffer.iter().enumerate() {
        if i > 0 {
            combined_payload.push_str("\n\n");
        }
        combined_payload.push_str(&msg.payload);
    }

    // Concatenate images
    let mut combined_images = Vec::new();
    for msg in &session.buffer {
        combined_images.extend(msg.images.clone());
    }

    IngressMessage {
        id: uuid::Uuid::new_v4(),
        source: source.to_string(),
        payload: combined_payload,
        trust,
        timestamp: earliest_ts,
        images: combined_images,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingress::types::TrustLevel;

    #[tokio::test]
    async fn test_debounce_single_message() {
        let config = IngressDebounceConfig {
            debounce_seconds: 1,
            max_debounce_messages: 50,
            max_debounce_chars: 100_000,
        };
        let mut manager = DebounceManager::new(config);
        let msg = IngressMessage::new(
            "source1".to_string(),
            "hello".to_string(),
            TrustLevel::OwnerCommand,
        );
        manager.ingest(msg);
        // Not enough time passed; flush_ready should return empty
        let now = Instant::now();
        let ready = manager.flush_ready(now);
        assert_eq!(ready.len(), 0);
        // Flush all should return the message
        let all = manager.flush_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].payload, "hello");
    }

    #[tokio::test]
    async fn test_debounce_multiple_messages_combined() {
        let config = IngressDebounceConfig {
            debounce_seconds: 2,
            max_debounce_messages: 50,
            max_debounce_chars: 100_000,
        };
        let mut manager = DebounceManager::new(config);
        // Ingest three messages quickly
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "first".to_string(),
            TrustLevel::OwnerCommand,
        ));
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "second".to_string(),
            TrustLevel::OwnerCommand,
        ));
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "third".to_string(),
            TrustLevel::OwnerCommand,
        ));

        // Still within debounce period
        let now = Instant::now();
        let ready = manager.flush_ready(now);
        assert_eq!(ready.len(), 0);

        // Advance time by > debounce_seconds
        let future = now + Duration::from_secs(3);
        let ready = manager.flush_ready(future);
        assert_eq!(ready.len(), 1);
        let combined = &ready[0];
        assert_eq!(combined.payload, "first\n\nsecond\n\nthird");
        assert_eq!(combined.source, "src");
    }

    #[tokio::test]
    async fn test_debounce_limit_enforcement() {
        let config = IngressDebounceConfig {
            debounce_seconds: 10,
            max_debounce_messages: 2,
            max_debounce_chars: 100_000,
        };
        let mut manager = DebounceManager::new(config);
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "a".to_string(),
            TrustLevel::OwnerCommand,
        ));
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "b".to_string(),
            TrustLevel::OwnerCommand,
        ));
        // At this, buffer len = 2, not >2 yet.
        let now = Instant::now();
        let ready = manager.flush_ready(now);
        assert_eq!(ready.len(), 0);
        // Third message should trigger limit exceed
        manager.ingest(IngressMessage::new(
            "src".to_string(),
            "c".to_string(),
            TrustLevel::OwnerCommand,
        ));
        // Now buffer has 3 > limit 2, so session should be marked ready immediately
        let ready = manager.flush_ready(Instant::now());
        // Since we marked last_update in past, flush_ready should flush it now
        assert_eq!(ready.len(), 1);
        let combined = &ready[0];
        // The combined should include all three? Actually after third ingest, we mark session ready, but buffer contains all three.
        assert_eq!(combined.payload, "a\n\nb\n\nc");
    }
}
