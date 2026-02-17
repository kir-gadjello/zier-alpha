use crate::agent::ImageAttachment;
use crate::config::Config;
use crate::ingress::{
    ApprovalCoordinator, ApprovalUIRequest, IngressBus, IngressMessage, RealTelegramClient,
    TelegramApi, TelegramClient, TrustLevel,
};
use crate::server::audio::AudioTranscriber;
use anyhow;
use base64::{engine::general_purpose, Engine as _};
use serde_json::json;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration, Instant};
use tracing::{debug, error, info, warn};

pub struct TelegramPollingService {
    config: Config,
    bus: Arc<IngressBus>,
    client: Arc<dyn TelegramApi>,
    project_dir: PathBuf,
    transcriber: Option<Box<dyn AudioTranscriber>>,
    approval_coord: Arc<ApprovalCoordinator>,
    approval_ui_rx: mpsc::Receiver<ApprovalUIRequest>,
}

impl TelegramPollingService {
    /// Create a new instance.
    /// `client` can be provided for testing; if None, a client is built from config's bot token.
    pub fn new(
        config: Config,
        bus: Arc<IngressBus>,
        project_dir: PathBuf,
        approval_coord: Arc<ApprovalCoordinator>,
        approval_ui_rx: mpsc::Receiver<ApprovalUIRequest>,
        client: Option<Arc<dyn TelegramApi>>,
    ) -> Option<Self> {
        let client = match client {
            Some(c) => c,
            None => {
                let token = config.server.telegram_bot_token.as_ref()?.clone();
                Arc::new(RealTelegramClient::new(token))
            }
        };
        let transcriber = match crate::server::audio::create_transcriber(&config) {
            Ok(t) => t,
            Err(e) => {
                error!("Failed to create audio transcriber: {}", e);
                None
            }
        };

        Some(Self {
            config,
            bus,
            client,
            project_dir,
            transcriber,
            approval_coord,
            approval_ui_rx,
        })
    }

    pub async fn run(mut self) {
        info!("Starting Telegram long polling service");

        // Spawn cleanup task for timed-out approvals
        let cleanup_coord = self.approval_coord.clone();
        let cleanup_client = self.client.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                let now = Instant::now();
                let expired = cleanup_coord.cleanup(now).await;
                for (call_id, chat_id, message_id) in expired {
                    debug!("Cleaning up timed-out approval: {}", call_id);
                    // Edit message to show timeout
                    let _ = cleanup_client
                        .edit_message_text(chat_id, message_id, "⌛️ Timed out")
                        .await;
                }
            }
        });

        let mut offset: Option<i64> = None;
        let timeout = self.config.server.telegram_poll_timeout;
        let mut backoff_secs = 1;

        loop {
            // Prepare the get_updates future
            let poll_fut = self.client.get_updates(offset, timeout);

            tokio::select! {
                updates = poll_fut => {
                    match updates {
                        Ok(updates) => {
                            // Reset backoff on success
                            backoff_secs = 1;

                            for update in updates {
                                // Update offset to the next update_id
                                offset = Some(update.update_id + 1);

                                // Handle message
                                if let Some(message) = update.message {
                                    if let Err(e) = self.handle_message(message).await {
                                        error!("Failed to handle Telegram message: {}", e);
                                    }
                                }

                                // Handle callback query (for approvals)
                                if let Some(callback_query) = update.callback_query {
                                    if let Err(e) = self.handle_callback_query(&callback_query).await {
                                        error!("Failed to handle callback query: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Telegram polling error: {}. Will retry...",
                                e
                            );
                            // Sleep before next poll (exponential backoff)
                            sleep(Duration::from_secs(backoff_secs)).await;
                            backoff_secs = (backoff_secs * 2).min(60);
                        }
                    }
                }
                ui_req = self.approval_ui_rx.recv() => {
                    match ui_req {
                        Some(req) => {
                            if let Err(e) = self.handle_approval_ui_request(req).await {
                                error!("Failed to handle approval UI request: {}", e);
                            }
                        }
                        None => {
                            warn!("Approval UI channel closed, stopping Telegram polling");
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn handle_document(
        &self,
        file_id: &str,
        file_name: Option<&str>,
        mime_type: Option<&str>,
        file_size: Option<i64>,
        message: &crate::ingress::TelegramMessage,
        chat_id: i64,
        trust: TrustLevel,
    ) -> anyhow::Result<()> {
        // Check if attachments are enabled
        if !self.config.server.attachments.enabled {
            return Ok(());
        }

        // Check file size limit
        if let Some(size) = file_size {
            if size > self.config.server.attachments.max_file_size_bytes as i64 {
                warn!(
                    "Attachment too large: {} > {} bytes",
                    size, self.config.server.attachments.max_file_size_bytes
                );
                return Ok(());
            }
        }

        // Generate safe filename
        let original_name = file_name.unwrap_or("file");
        let safe_name: String = original_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let filename = format!("{}_{}_{}", message.message_id, chat_id, safe_name);

        // Prepare attachment directory
        let base_dir = &self.config.server.attachments.base_dir;
        let attach_dir = self.project_dir.join(base_dir).join("telegram");
        if let Err(e) = fs::create_dir_all(&attach_dir).await {
            error!("Failed to create attachments directory: {}", e);
            return Ok(());
        }
        let file_path = attach_dir.join(&filename);

        // Download file
        let download_url = self.client.get_file_download_url(file_id).await?;
        let bytes = self.client.download_file(&download_url).await?;

        // Write file to disk
        let mut file = fs::File::create(&file_path).await?;
        file.write_all(&bytes).await?;

        // Build XML armor block
        let relative_path = format!("{}/telegram/{}", base_dir, filename);
        let xml = format!(
            r#"<context>
<attached-file filename="{}" mime="{}" size="{}" path="{}"/>
</context>"#,
            original_name,
            mime_type.unwrap_or("application/octet-stream"),
            bytes.len(),
            relative_path
        );

        // Combine with caption if any
        let final_text = if let Some(caption) = &message.caption {
            format!("{}\n\n{}", caption, xml)
        } else {
            xml
        };

        // Push to ingress bus
        let msg = IngressMessage::new(format!("telegram:{}", chat_id), final_text, trust);
        self.bus.push(msg).await?;
        debug!(
            "Attachment saved and pushed for message {}",
            message.message_id
        );

        Ok(())
    }

    async fn handle_message(&self, message: crate::ingress::TelegramMessage) -> anyhow::Result<()> {
        let user = match &message.from {
            Some(u) => u,
            None => return Ok(()),
        };
        let chat_id = user.id;
        let trust = if let Some(owner_id) = self.config.server.owner_telegram_id {
            if user.id == owner_id {
                TrustLevel::OwnerCommand
            } else {
                TrustLevel::UntrustedEvent
            }
        } else {
            TrustLevel::UntrustedEvent
        };

        // 1. Handle document attachments
        if let Some(doc) = &message.document {
            return self
                .handle_document(
                    &doc.file_id,
                    doc.file_name.as_deref(),
                    doc.mime_type.as_deref(),
                    doc.file_size,
                    &message,
                    chat_id,
                    trust,
                )
                .await;
        }

        // 2. Handle audio/voice with optional transcription
        if let Some(audio) = &message.audio {
            if let Some(transcriber) = &self.transcriber {
                // Download audio bytes
                let download_url = match self.client.get_file_download_url(&audio.file_id).await {
                    Ok(url) => url,
                    Err(e) => {
                        error!("Failed to get download URL for audio: {}", e);
                        return Ok(());
                    }
                };
                let bytes = match self.client.download_file(&download_url).await {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Failed to download audio: {}", e);
                        return Ok(());
                    }
                };
                // Write to temporary file
                let mut temp_file = match NamedTempFile::new() {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Failed to create temp file: {}", e);
                        return Ok(());
                    }
                };
                if let Err(e) = temp_file.write_all(&bytes) {
                    error!("Failed to write temp file: {}", e);
                    return Ok(());
                }
                if let Err(e) = temp_file.flush() {
                    error!("Failed to flush temp file: {}", e);
                    return Ok(());
                }
                let temp_path = temp_file.path().to_path_buf();

                // Transcribe
                let transcript = match transcriber.transcribe(&temp_path).await {
                    Ok(text) => text,
                    Err(e) => {
                        error!("Transcription failed: {}", e);
                        // Fallback to document handling if transcription fails
                        return self
                            .handle_document(
                                &audio.file_id,
                                audio.file_name.as_deref(),
                                audio.mime_type.as_deref(),
                                audio.file_size,
                                &message,
                                chat_id,
                                trust,
                            )
                            .await;
                    }
                };

                // Build final message with caption if any
                let final_text = if let Some(caption) = &message.caption {
                    format!("{}\n\n{}", caption, transcript)
                } else {
                    transcript
                };

                let msg = IngressMessage::new(format!("telegram:{}", chat_id), final_text, trust);
                self.bus.push(msg).await?;
                debug!(
                    "Audio transcription completed for message {}",
                    message.message_id
                );
                return Ok(());
            } else {
                // No transcriber configured, fallback to document handling
                return self
                    .handle_document(
                        &audio.file_id,
                        audio.file_name.as_deref(),
                        audio.mime_type.as_deref(),
                        audio.file_size,
                        &message,
                        chat_id,
                        trust,
                    )
                    .await;
            }
        }
        // Voice handling (similar)
        if let Some(voice) = &message.voice {
            if let Some(transcriber) = &self.transcriber {
                let download_url = match self.client.get_file_download_url(&voice.file_id).await {
                    Ok(url) => url,
                    Err(e) => {
                        error!("Failed to get download URL for voice: {}", e);
                        return Ok(());
                    }
                };
                let bytes = match self.client.download_file(&download_url).await {
                    Ok(b) => b,
                    Err(e) => {
                        error!("Failed to download voice: {}", e);
                        return Ok(());
                    }
                };
                let mut temp_file = match NamedTempFile::new() {
                    Ok(f) => f,
                    Err(e) => {
                        error!("Failed to create temp file: {}", e);
                        return Ok(());
                    }
                };
                if let Err(e) = temp_file.write_all(&bytes) {
                    error!("Failed to write temp file: {}", e);
                    return Ok(());
                }
                if let Err(e) = temp_file.flush() {
                    error!("Failed to flush temp file: {}", e);
                    return Ok(());
                }
                let temp_path = temp_file.path().to_path_buf();

                let transcript = match transcriber.transcribe(&temp_path).await {
                    Ok(text) => text,
                    Err(e) => {
                        error!("Transcription failed: {}", e);
                        return self
                            .handle_document(
                                &voice.file_id,
                                None,
                                voice.mime_type.as_deref(),
                                voice.file_size,
                                &message,
                                chat_id,
                                trust,
                            )
                            .await;
                    }
                };

                let final_text = if let Some(caption) = &message.caption {
                    format!("{}\n\n{}", caption, transcript)
                } else {
                    transcript
                };

                let msg = IngressMessage::new(format!("telegram:{}", chat_id), final_text, trust);
                self.bus.push(msg).await?;
                debug!(
                    "Voice transcription completed for message {}",
                    message.message_id
                );
                return Ok(());
            } else {
                return self
                    .handle_document(
                        &voice.file_id,
                        None,
                        voice.mime_type.as_deref(),
                        voice.file_size,
                        &message,
                        chat_id,
                        trust,
                    )
                    .await;
            }
        }

        // 3. Handle photos (existing)
        let (payload, images) = if let Some(photos) = &message.photo {
            if let Some(largest) = photos.last() {
                match self.client.get_file_download_url(&largest.file_id).await {
                    Ok(url) => match self.client.download_file(&url).await {
                        Ok(bytes) => {
                            let b64 = general_purpose::STANDARD.encode(&bytes);
                            let img = ImageAttachment {
                                data: b64,
                                media_type: "image/jpeg".to_string(),
                            };
                            // Use caption if present, else default text
                            let text = message
                                .caption
                                .clone()
                                .unwrap_or_else(|| "User sent an image".to_string());
                            (Some(text), vec![img])
                        }
                        Err(e) => {
                            warn!("Failed to download image bytes: {}", e);
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
        } else if let Some(text) = &message.text {
            (Some(text.clone()), Vec::new())
        } else {
            (None, Vec::new())
        };

        // Push to bus if we have a text payload
        if let Some(text) = payload {
            let msg = IngressMessage::new(format!("telegram:{}", chat_id), text, trust)
                .with_images(images);
            self.bus.push(msg).await?;
            debug!("Pushed polled Telegram message from {} to bus", user.id);
        }

        Ok(())
    }

    async fn handle_approval_ui_request(&self, req: ApprovalUIRequest) -> anyhow::Result<()> {
        use crate::ingress::ApprovalDecision;
        // Build text
        let text = format!(
            "Tool `{}` requires approval:\nArguments: `{}`",
            req.tool_name, req.arguments
        );
        // Send message with buttons
        let message_id = self
            .client
            .send_approval_message(req.chat_id, &text, &req.call_id)
            .await?;
        // Respond with message_id
        if req.respond_msg_id.send(message_id).is_err() {
            warn!("Failed to send message_id response for approval UI request");
        }
        Ok(())
    }

    async fn handle_callback_query(
        &self,
        query: &crate::ingress::TelegramCallbackQuery,
    ) -> anyhow::Result<()> {
        use crate::ingress::ApprovalDecision;
        let data = query
            .data
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing callback data"))?;
        let parts: Vec<&str> = data.splitn(2, ':').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid callback data format: {}", data);
        }
        let (decision_str, call_id) = (parts[0], parts[1]);
        let decision = match decision_str {
            "approve" => ApprovalDecision::Approve,
            "deny" => ApprovalDecision::Deny,
            _ => anyhow::bail!("Unknown decision: {}", decision_str),
        };
        // Resolve via coordinator
        if let Some((chat_id, message_id)) = self.approval_coord.resolve(call_id, decision).await {
            // Edit original message to show result
            let result_text = match decision {
                ApprovalDecision::Approve => "✅ Approved",
                ApprovalDecision::Deny => "❌ Denied",
            };
            if let Err(e) = self
                .client
                .edit_message_text(chat_id, message_id, result_text)
                .await
            {
                error!("Failed to edit message for approval result: {}", e);
            }
        }
        // Answer callback query to remove loading indicator
        if let Err(e) = self
            .client
            .answer_callback_query(&query.id, Some("Processed"))
            .await
        {
            error!("Failed to answer callback query: {}", e);
        }
        Ok(())
    }

    /// Helper used in tests to process a single Telegram message.
    pub async fn process_message_for_test(
        &self,
        message: crate::ingress::TelegramMessage,
    ) -> anyhow::Result<()> {
        self.handle_message(message).await
    }

    /// Test‑only helper to simulate handling an ApprovalUIRequest.
    pub async fn process_approval_ui(&self, req: ApprovalUIRequest) -> anyhow::Result<()> {
        self.handle_approval_ui_request(req).await
    }
}
