use axum::{
    extract::{State, Json},
    http::{StatusCode, HeaderMap},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use std::sync::Arc;
use tracing::{info, warn};
use crate::ingress::{IngressMessage, TrustLevel};
use crate::server::http::AppState;

// Telegram Update structure (partial)
#[derive(serde::Deserialize)]
struct Update {
    message: Option<Message>,
}

#[derive(serde::Deserialize)]
struct Message {
    from: Option<User>,
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct User {
    id: i64,
}

pub async fn webhook_handler(
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
    Json(update): Json<Value>,
) -> Response {
    // 1. Verify Secret Token
    if let Some(expected_token) = &state.config.server.telegram_secret_token {
        if let Some(received_token) = headers.get("x-telegram-bot-api-secret-token") {
            if received_token != expected_token {
                warn!("Invalid Telegram secret token");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        } else {
            warn!("Missing Telegram secret token");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    }

    let bus = if let Some(bus) = &state.bus {
        bus
    } else {
        warn!("Telegram webhook called but IngressBus is not initialized");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };

    // 2. Parse Update
    let update: Update = match serde_json::from_value(update) {
        Ok(u) => u,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Some(message) = update.message {
        if let Some(text) = message.text {
            if let Some(user) = message.from {
                let trust = if let Some(owner_id) = state.config.server.owner_telegram_id {
                    if user.id == owner_id {
                        TrustLevel::OwnerCommand
                    } else {
                        TrustLevel::UntrustedEvent
                    }
                } else {
                    // No owner configured -> Untrusted
                    TrustLevel::UntrustedEvent
                };

                let msg = IngressMessage::new(
                    format!("telegram:{}", user.id),
                    text,
                    trust,
                );

                if let Err(e) = bus.push(msg).await {
                    warn!("Failed to push telegram message to bus: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }

                info!("Pushed Telegram message from {} to bus (Trust: {:?})", user.id, trust);
            }
        }
    }

    StatusCode::OK.into_response()
}
