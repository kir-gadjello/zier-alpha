use crate::agent::ImageAttachment;
use crate::config::Config;
use crate::ingress::{IngressBus, IngressMessage, TelegramClient, TrustLevel};
use base64::{engine::general_purpose, Engine as _};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

pub struct TelegramPollingService {
    config: Config,
    bus: Arc<IngressBus>,
    client: TelegramClient,
}

impl TelegramPollingService {
    pub fn new(config: Config, bus: Arc<IngressBus>) -> Option<Self> {
        let token = config.server.telegram_bot_token.as_ref()?.clone();
        let client = TelegramClient::new(token);

        Some(Self {
            config,
            bus,
            client,
        })
    }

    pub async fn run(&self) {
        info!("Starting Telegram long polling service");

        let mut offset: Option<i64> = None;
        let timeout = self.config.server.telegram_poll_timeout;
        let mut backoff_secs = 1;

        loop {
            match self.client.get_updates(offset, timeout).await {
                Ok(updates) => {
                    // Reset backoff on success
                    backoff_secs = 1;

                    for update in updates {
                        // Update offset to the next update_id
                        offset = Some(update.update_id + 1);

                        if let Some(message) = update.message {
                            if let Err(e) = self.handle_message(message).await {
                                error!("Failed to handle Telegram message: {}", e);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "Telegram polling error: {}. Retrying in {}s...",
                        e, backoff_secs
                    );
                    sleep(Duration::from_secs(backoff_secs)).await;
                    // Exponential backoff up to 60s
                    backoff_secs = (backoff_secs * 2).min(60);
                }
            }
        }
    }

    async fn handle_message(&self, message: crate::ingress::TelegramMessage) -> anyhow::Result<()> {
        let (payload, images) = if let Some(text) = message.text {
            (Some(text), Vec::new())
        } else if let Some(photos) = message.photo {
            if let Some(largest) = photos.last() {
                match self.client.get_file_download_url(&largest.file_id).await {
                    Ok(url) => match reqwest::get(&url).await {
                        Ok(resp) => match resp.bytes().await {
                            Ok(bytes) => {
                                let b64 = general_purpose::STANDARD.encode(&bytes);
                                let img = ImageAttachment {
                                    data: b64,
                                    media_type: "image/jpeg".to_string(),
                                };
                                (Some("User sent an image".to_string()), vec![img])
                            }
                            Err(e) => {
                                warn!("Failed to download image bytes: {}", e);
                                (None, Vec::new())
                            }
                        },
                        Err(e) => {
                            warn!("Failed to download image: {}", e);
                            (None, Vec::new())
                        }
                    },
                    Err(e) => {
                        warn!("Failed to get file url: {}", e);
                        (None, Vec::new())
                    }
                }
            } else {
                (None, Vec::new())
            }
        } else {
            (None, Vec::new())
        };

        if let Some(text) = payload {
            if let Some(user) = message.from {
                let trust = if let Some(owner_id) = self.config.server.owner_telegram_id {
                    if user.id == owner_id {
                        TrustLevel::OwnerCommand
                    } else {
                        TrustLevel::UntrustedEvent
                    }
                } else {
                    TrustLevel::UntrustedEvent
                };

                let msg = IngressMessage::new(format!("telegram:{}", user.id), text, trust)
                    .with_images(images);

                self.bus.push(msg).await?;
                debug!("Pushed polled Telegram message from {} to bus", user.id);
            }
        }

        Ok(())
    }
}
