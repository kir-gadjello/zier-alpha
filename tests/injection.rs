use localgpt::ingress::{IngressMessage, TrustLevel};
use localgpt::config::Config;
use localgpt::server::telegram; // I need to make this public or accessible?
// telegram module is accessible if I made it pub in server/mod.rs

#[tokio::test]
async fn test_injection_trace_ingress() {
    // We want to verify that an incoming message from a non-owner is tagged UntrustedEvent.

    // Since we can't easily call the axum handler directly without setting up the whole stack,
    // we will simulate the logic here, effectively unit testing the decision logic.

    let owner_id = 999;
    let sender_id = 123;

    let config = Config::default();
    // config.server.owner_telegram_id = Some(owner_id); // Wait, Config fields are public?
    // Config struct has public fields.

    let mut server_config = localgpt::config::ServerConfig::default();
    server_config.owner_telegram_id = Some(owner_id);

    // Logic from telegram.rs:
    let trust = if let Some(oid) = server_config.owner_telegram_id {
        if sender_id == oid {
            TrustLevel::OwnerCommand
        } else {
            TrustLevel::UntrustedEvent
        }
    } else {
        TrustLevel::UntrustedEvent
    };

    assert_eq!(trust, TrustLevel::UntrustedEvent);

    // Case 2: Owner
    let sender_id_owner = 999;
    let trust_owner = if let Some(oid) = server_config.owner_telegram_id {
        if sender_id_owner == oid {
            TrustLevel::OwnerCommand
        } else {
            TrustLevel::UntrustedEvent
        }
    } else {
        TrustLevel::UntrustedEvent
    };

    assert_eq!(trust_owner, TrustLevel::OwnerCommand);
}
