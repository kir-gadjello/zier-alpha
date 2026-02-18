use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{debug, error};

/// Trait abstracting Telegram client operations for testability.
#[async_trait]
pub trait TelegramApi: Send + Sync {
    async fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<TelegramUpdate>>;
    async fn send_message(&self, chat_id: i64, text: &str) -> Result<()>;
    async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<()>;
    async fn answer_callback_query(&self, query_id: &str, text: Option<&str>) -> Result<()>;
    async fn get_file_download_url(&self, file_id: &str) -> Result<String>;
    async fn download_file(&self, url: &str) -> Result<Vec<u8>>;
    async fn send_approval_message(&self, chat_id: i64, text: &str, call_id: &str) -> Result<i64>;
}

/// Adapter from the concrete `TelegramClient` to the `TelegramApi` trait.
pub struct RealTelegramClient(TelegramClient);

impl RealTelegramClient {
    pub fn new(bot_token: String) -> Self {
        Self(TelegramClient::new(bot_token))
    }
}

// Ensure thread-safety for trait objects
unsafe impl Send for RealTelegramClient {}
unsafe impl Sync for RealTelegramClient {}

#[async_trait]
impl TelegramApi for RealTelegramClient {
    async fn get_updates(&self, offset: Option<i64>, timeout: u64) -> Result<Vec<TelegramUpdate>> {
        self.0.get_updates(offset, timeout).await
    }
    async fn send_message(&self, chat_id: i64, text: &str) -> Result<()> {
        self.0.send_message(chat_id, text).await
    }
    async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<()> {
        self.0.edit_message_text(chat_id, message_id, text).await
    }
    async fn answer_callback_query(&self, query_id: &str, text: Option<&str>) -> Result<()> {
        self.0.answer_callback_query(query_id, text).await
    }
    async fn get_file_download_url(&self, file_id: &str) -> Result<String> {
        self.0.get_file_download_url(file_id).await
    }
    async fn download_file(&self, url: &str) -> Result<Vec<u8>> {
        self.0.download_file(url).await
    }
    async fn send_approval_message(&self, chat_id: i64, text: &str, call_id: &str) -> Result<i64> {
        self.0.send_approval_message(chat_id, text, call_id).await
    }
}

/// Concrete Telegram client.
#[derive(Clone)]
pub struct TelegramClient {
    client: Client,
    bot_token: String,
    api_base: String,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(70))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            bot_token,
            api_base: "https://api.telegram.org".to_string(),
        }
    }

    pub async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: u64,
    ) -> Result<Vec<TelegramUpdate>> {
        let url = format!("{}/bot{}/getUpdates", self.api_base, self.bot_token);
        let mut body = json!({ "timeout": timeout });
        if let Some(off) = offset {
            body["offset"] = json!(off);
        }
        debug!(
            "Polling Telegram updates (offset={:?}, timeout={}s)",
            offset, timeout
        );
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
        let max_len = 4000;
        let text_chars: Vec<char> = text.chars().collect();
        let mut start = 0;
        while start < text_chars.len() {
            let end = (start + max_len).min(text_chars.len());
            let chunk: String = text_chars[start..end].iter().collect();
            let body = json!({ "chat_id": chat_id, "text": chunk });
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
            Ok(format!(
                "{}/file/bot{}/{}",
                self.api_base, self.bot_token, path
            ))
        } else {
            anyhow::bail!("Failed to get file path from Telegram: {:?}", json);
        }
    }

    pub async fn download_file(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.client.get(url).send().await?;
        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Failed to download file: {}", err);
        }
        let bytes = resp.bytes().await?;
        Ok(bytes.to_vec())
    }

    pub async fn send_approval_message(
        &self,
        chat_id: i64,
        text: &str,
        call_id: &str,
    ) -> Result<i64> {
        let url = format!("{}/bot{}/sendMessage", self.api_base, self.bot_token);
        let reply_markup = json!({
            "inline_keyboard": [
                [
                    { "text": "✅ Approve", "callback_data": format!("approve:{}", call_id) },
                    { "text": "❌ Deny", "callback_data": format!("deny:{}", call_id) }
                ]
            ]
        });
        let body = json!({ "chat_id": chat_id, "text": text, "reply_markup": reply_markup });
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram sendMessage error: {}", err);
        }
        let json: serde_json::Value = resp.json().await?;
        let message_id = json["result"]["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("No message_id in response"))?;
        Ok(message_id)
    }

    pub async fn edit_message_text(&self, chat_id: i64, message_id: i64, text: &str) -> Result<()> {
        let url = format!("{}/bot{}/editMessageText", self.api_base, self.bot_token);
        let body = json!({ "chat_id": chat_id, "message_id": message_id, "text": text });
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram editMessageText error: {}", err);
        }
        Ok(())
    }

    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/bot{}/answerCallbackQuery",
            self.api_base, self.bot_token
        );
        let mut body = json!({ "callback_query_id": callback_query_id });
        if let Some(t) = text {
            body["text"] = json!(t);
        }
        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let err = resp.text().await?;
            anyhow::bail!("Telegram answerCallbackQuery error: {}", err);
        }
        Ok(())
    }

    /// Perform a GET request using the client with configured timeout.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response> {
        self.client.get(url).send().await.map_err(Into::into)
    }
}

// Data structures
#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUpdate {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    #[serde(rename = "callback_query")]
    pub callback_query: Option<TelegramCallbackQuery>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TelegramDocument {
    pub file_id: String,
    #[serde(rename = "file_name")]
    pub file_name: Option<String>,
    #[serde(rename = "mime_type")]
    pub mime_type: Option<String>,
    #[serde(rename = "file_size")]
    pub file_size: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TelegramAudio {
    pub file_id: String,
    #[serde(rename = "file_name")]
    pub file_name: Option<String>,
    #[serde(rename = "mime_type")]
    pub mime_type: Option<String>,
    #[serde(rename = "file_size")]
    pub file_size: Option<i64>,
    #[serde(rename = "duration")]
    pub duration: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TelegramVoice {
    pub file_id: String,
    #[serde(rename = "mime_type")]
    pub mime_type: Option<String>,
    #[serde(rename = "file_size")]
    pub file_size: Option<i64>,
    #[serde(rename = "duration")]
    pub duration: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
    pub photo: Option<Vec<TelegramPhotoSize>>,
    pub document: Option<TelegramDocument>,
    pub audio: Option<TelegramAudio>,
    pub voice: Option<TelegramVoice>,
    pub caption: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramUser {
    pub id: i64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TelegramCallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    #[serde(rename = "message")]
    pub message: Option<TelegramMessage>,
    #[serde(rename = "data")]
    pub data: Option<String>,
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
