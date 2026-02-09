use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error};

#[derive(Clone)]
pub struct TelegramClient {
    client: Client,
    bot_token: String,
    api_base: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
    pub photo: Option<Vec<TelegramPhotoSize>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUser {
    pub id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramPhotoSize {
    pub file_id: String,
}

#[derive(Debug, Deserialize)]
struct GetUpdatesResponse {
    pub ok: bool,
    pub result: Vec<TelegramUpdate>,
}

use std::time::Duration;

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        // Set a long timeout for the client to accommodate long polling (e.g., 60s + buffer)
        // Telegram max timeout is usually 50-60s.
        let client = Client::builder()
            .timeout(Duration::from_secs(70))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            bot_token,
            api_base: "https://api.telegram.org".to_string(),
        }
    }

    pub async fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<TelegramUpdate>> {
        let url = format!("{}/bot{}/getUpdates", self.api_base, self.bot_token);
        let mut body = json!({
            "timeout": timeout,
        });

        if let Some(off) = offset {
            body["offset"] = json!(off);
        }

        debug!("Polling Telegram updates (offset={:?}, timeout={}s)", offset, timeout);

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let err_text = resp.text().await?;
            error!("Telegram getUpdates error: {}", err_text);
            anyhow::bail!("Telegram getUpdates failed: {}", err_text);
        }

        let data: GetUpdatesResponse = resp.json().await?;
        if !data.ok {
            anyhow::bail!("Telegram getUpdates returned ok=false");
        }

        Ok(data.result)
    }

    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<()> {
        let url = format!("{}/bot{}/sendMessage", self.api_base, self.bot_token);

        // Telegram message length limit is 4096 chars
        let max_len = 4000;
        let text_chars: Vec<char> = text.chars().collect();
        let mut start = 0;

        while start < text_chars.len() {
            let end = (start + max_len).min(text_chars.len());
            let chunk: String = text_chars[start..end].iter().collect();

            let body = json!({
                "chat_id": chat_id,
                "text": chunk,
            });

            let resp = self.client.post(&url).json(&body).send().await?;
            if !resp.status().is_success() {
                let err_text = resp.text().await?;
                error!("Telegram sendMessage error: {}", err_text);
                anyhow::bail!("Telegram sendMessage failed: {}", err_text);
            }
            start = end;
        }
        Ok(())
    }

    pub async fn get_file_download_url(&self, file_id: &str) -> Result<String> {
        let url = format!("{}/bot{}/getFile", self.api_base, self.bot_token);
        let body = json!({ "file_id": file_id });

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
             let err = resp.text().await?;
             anyhow::bail!("Telegram getFile failed: {}", err);
        }

        let json: serde_json::Value = resp.json().await?;

        if let Some(path) = json["result"]["file_path"].as_str() {
             Ok(format!("{}/file/bot{}/{}", self.api_base, self.bot_token, path))
        } else {
             anyhow::bail!("Failed to get file path from Telegram: {:?}", json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_telegram_update() {
        let json = r#"{
            "update_id": 100,
            "message": {
                "message_id": 1,
                "from": { "id": 123 },
                "text": "hello"
            }
        }"#;
        let update: TelegramUpdate = serde_json::from_str(json).unwrap();
        assert_eq!(update.update_id, 100);
        assert_eq!(update.message.unwrap().text.unwrap(), "hello");
    }

    #[test]
    fn test_parse_get_updates_response() {
        let json = r#"{
            "ok": true,
            "result": [
                {
                    "update_id": 100,
                    "message": {
                        "message_id": 1,
                        "from": { "id": 123 },
                        "text": "hello"
                    }
                }
            ]
        }"#;
        let resp: GetUpdatesResponse = serde_json::from_str(json).unwrap();
        assert!(resp.ok);
        assert_eq!(resp.result.len(), 1);
        assert_eq!(resp.result[0].update_id, 100);
    }
}
