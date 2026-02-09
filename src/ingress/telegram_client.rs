use anyhow::Result;
use reqwest::Client;
use serde_json::json;
use tracing::error;

#[derive(Clone)]
pub struct TelegramClient {
    client: Client,
    bot_token: String,
    api_base: String,
}

impl TelegramClient {
    pub fn new(bot_token: String) -> Self {
        Self {
            client: Client::new(),
            bot_token,
            api_base: "https://api.telegram.org".to_string(),
        }
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
