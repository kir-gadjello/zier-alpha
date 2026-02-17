/// Audio transcription support for Telegram voice/audio messages.
///
/// Provides a trait-based abstraction over different STT backends:
/// - Local command (e.g., whisper-cpp)
/// - OpenAI Whisper API
/// - Gemini API
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command as TokioCommand;

#[async_trait]
pub trait AudioTranscriber: Send + Sync {
    async fn transcribe(&self, path: &Path) -> Result<String>;
}

/// Local command transcriber using a shell command template.
/// The template should contain `{}` placeholder for the file path.
pub struct LocalCommandTranscriber {
    command_template: String,
}

impl LocalCommandTranscriber {
    pub fn new(command_template: String) -> Self {
        Self { command_template }
    }
}

#[async_trait]
impl AudioTranscriber for LocalCommandTranscriber {
    async fn transcribe(&self, path: &Path) -> Result<String> {
        let path_str = path.to_str().context("Invalid audio file path")?;
        let command = self.command_template.replace("{}", path_str);
        // Use tokio::process::Command for async execution
        let output = TokioCommand::new("sh")
            .arg("-c")
            .arg(&command)
            .output()
            .await?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Local transcription command failed: {}", err);
        }
        let stdout = String::from_utf8(output.stdout)?;
        Ok(stdout)
    }
}

/// OpenAI Whisper transcriber.
pub struct OpenAITranscriber {
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OpenAITranscriber {
    pub fn new(api_key: String, base_url: Option<&str>, model: Option<&str>) -> Self {
        let client = reqwest::Client::new();
        Self {
            api_key,
            base_url: base_url.unwrap_or("https://api.openai.com/v1").to_string(),
            model: model.unwrap_or("whisper-1").to_string(),
            client,
        }
    }
}

#[async_trait]
impl AudioTranscriber for OpenAITranscriber {
    async fn transcribe(&self, path: &Path) -> Result<String> {
        let form = reqwest::multipart::Form::new()
            .file("file", path)
            .await
            .context("Failed to attach file to multipart form")?
            .text("model", self.model.clone());

        let response = self
            .client
            .post(&format!("{}/audio/transcriptions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("OpenAI Whisper API error {}: {}", status, text);
        }

        let json: serde_json::Value = response.json().await?;
        let text = json
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("No 'text' field in OpenAI response")?;
        Ok(text)
    }
}

/// Gemini transcriber (similar to OpenAI, using Gemini's API).
/// For now, we use the same structure but different endpoint.
pub struct GeminiTranscriber {
    api_key: String,
    base_url: String, // e.g., "https://generativelanguage.googleapis.com/v1beta"
    model: String,    // e.g., "gemini-2.0-flash-exp"
    client: reqwest::Client,
}

impl GeminiTranscriber {
    pub fn new(api_key: String, base_url: Option<&str>, model: Option<&str>) -> Self {
        Self {
            api_key,
            base_url: base_url
                .unwrap_or("https://generativelanguage.googleapis.com/v1beta")
                .to_string(),
            model: model.unwrap_or("gemini-2.0-flash-exp").to_string(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl AudioTranscriber for GeminiTranscriber {
    async fn transcribe(&self, path: &Path) -> Result<String> {
        // Use OpenAI-compatible multipart transcription endpoint.
        // Assumes the configured base_url is OpenAI-compatible (e.g., an API proxy for Gemini).
        let form = reqwest::multipart::Form::new()
            .file("file", path)
            .await
            .context("Failed to attach file to multipart form")?
            .text("model", self.model.clone());

        let response = self
            .client
            .post(&format!("{}/audio/transcriptions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("Gemini transcription API error {}: {}", status, text);
        }

        let json: serde_json::Value = response.json().await?;
        let text = json
            .get("text")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .context("No 'text' field in Gemini response")?;
        Ok(text)
    }
}

/// Factory function to create a transcriber based on config.
pub fn create_transcriber(
    config: &crate::config::Config,
) -> Result<Option<Box<dyn AudioTranscriber>>> {
    if !config.server.audio.enabled {
        return Ok(None);
    }
    let backend = &config.server.audio.backend;
    match backend.as_str() {
        "local" => {
            let cmd = config
                .server
                .audio
                .local_command
                .as_ref()
                .context("Local backend selected but no command configured")?;
            Ok(Some(Box::new(LocalCommandTranscriber::new(cmd.clone()))))
        }
        "openai" => {
            let api_key = config
                .providers
                .openai
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not configured"))?
                .api_key
                .clone();
            Ok(Some(Box::new(OpenAITranscriber::new(
                api_key,
                None,
                config.server.audio.openai_model.as_deref(),
            ))))
        }
        "gemini" => {
            let provider_cfg = config
                .providers
                .gemini
                .as_ref()
                .context("Gemini provider not configured")?;
            let model = config.server.audio.gemini_model.as_deref();
            Ok(Some(Box::new(GeminiTranscriber::new(
                provider_cfg.api_key.clone(),
                Some(&provider_cfg.base_url),
                model,
            ))))
        }
        _ => anyhow::bail!("Unknown audio backend: {}", backend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_local_command_transcriber() {
        // Create a temporary file with test content
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "test audio content").unwrap();
        let path = file.path().to_path_buf();

        // Use a simple command that cat's the file (just echoing content)
        let transcriber = LocalCommandTranscriber::new("cat {}".to_string());
        let result = transcriber.transcribe(&path).await.unwrap();
        assert_eq!(result, "test audio content\n");
    }
}
