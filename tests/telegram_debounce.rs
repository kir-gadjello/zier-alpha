// Integration test for Telegram debounce queue
// Tests that messages from the same source are combined and flushed after the debounce period,
// and that size limits enforce immediate flush.

use std::time::{Duration, Instant};
use tokio::time::sleep;
use uuid::Uuid;
use zier_alpha::config::IngressDebounceConfig;
use zier_alpha::ingress::debounce::DebounceManager;
use zier_alpha::ingress::{IngressMessage, TrustLevel};

fn make_msg(source: &str, payload: &str) -> IngressMessage {
    IngressMessage {
        id: Uuid::new_v4(),
        source: source.to_string(),
        payload: payload.to_string(),
        trust: TrustLevel::OwnerCommand,
        timestamp: 0,
        images: Vec::new(),
    }
}

#[tokio::test]
async fn test_debounce_timing_integration() {
    // Debounce config: 1 second quiet period
    let config = IngressDebounceConfig {
        debounce_seconds: 1,
        max_debounce_messages: 50,
        max_debounce_chars: 100_000,
    };
    let mut manager = DebounceManager::new(config);

    let source = "telegram:12345".to_string();

    // Ingest two messages in quick succession
    manager.ingest(make_msg(&source, "first part"));
    manager.ingest(make_msg(&source, "second part"));

    // Wait longer than debounce period (1.1 seconds)
    sleep(Duration::from_millis(1100)).await;

    let now = Instant::now();
    let ready = manager.flush_ready(now);
    assert_eq!(ready.len(), 1, "Expected exactly one combined message");
    let combined = &ready[0];
    assert_eq!(combined.source, source);
    assert!(combined.payload.contains("first part"));
    assert!(combined.payload.contains("second part"));
    assert!(combined.images.is_empty());
}

#[tokio::test]
async fn test_debounce_limit_integration() {
    // Config with very low message limit
    let config = IngressDebounceConfig {
        debounce_seconds: 10,
        max_debounce_messages: 2,
        max_debounce_chars: 100_000,
    };
    let mut manager = DebounceManager::new(config);

    let source = "telegram:999".to_string();

    // Ingest two messages (still below limit)
    manager.ingest(make_msg(&source, "a"));
    manager.ingest(make_msg(&source, "b"));

    // At this point, buffer length is 2, not >2, so not forced flush.
    let now = Instant::now();
    let ready = manager.flush_ready(now);
    assert_eq!(ready.len(), 0, "Should not flush yet");

    // Ingest third message; should exceed limit (2) and force flush
    manager.ingest(make_msg(&source, "c"));

    // Immediately flush - should be ready because last_update was set to past
    let now2 = Instant::now();
    let ready = manager.flush_ready(now2);
    assert_eq!(
        ready.len(),
        1,
        "Should flush immediately after exceeding limit"
    );
    let combined = &ready[0];
    assert_eq!(combined.payload, "a\n\nb\n\nc");
}
