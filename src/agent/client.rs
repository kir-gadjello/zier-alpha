use crate::agent::providers::{
    create_provider, AnthropicProvider, ClaudeCliProvider, LLMProvider, LLMResponse, Message,
    OllamaProvider, OpenAIProvider, StreamResult, ToolSchema,
};
use crate::agent::llm_error::LlmError;
use crate::config::{
    models::{resolve_model_config, ModelConfig},
    Config,
};
use anyhow::Result;
use async_trait::async_trait;
use std::env;
use std::time::Instant;
use tracing::warn;

pub struct SmartResponse {
    pub response: LLMResponse,
    pub used_model: String,
    pub provider_name: String,
    pub latency_ms: u64,
}

#[derive(Clone)]
pub struct SmartClient {
    config: Config,
    model_alias: String,
}

impl SmartClient {
    pub fn new(config: Config, model_alias: String) -> Self {
        Self {
            config,
            model_alias,
        }
    }

    pub fn resolve_config(&self, model_alias: &str) -> Result<ModelConfig> {
        if self.config.models.contains_key(model_alias) {
            resolve_model_config(model_alias, &self.config.models)
        } else {
            Ok(ModelConfig {
                provider: None,
                api_base: None,
                api_key_env: None,
                model: model_alias.to_string(),
                extend: None,
                timeout: None,
                extra_body: None,
                fallback_models: None,
                fallback_settings: None,
                aliases: None,
                supports_vision: None,
            })
        }
    }

    fn check_fallback_allowed(&self, error: &anyhow::Error, config: &ModelConfig) -> bool {
        let settings = match &config.fallback_settings {
            Some(s) => s,
            None => return true, // Default to allowing fallback if no settings
        };

        let err_str = error.to_string();

        // Extract status code from LlmError if available
        let status_code = if let Some(llm_err) = error.downcast_ref::<LlmError>() {
             if let Some(code) = llm_err.status_code() {
                 code.to_string()
             } else {
                 "500".to_string()
             }
        } else {
            // Default to 500 if unknown, or try parsing string (legacy behavior)
            err_str
            .split_whitespace()
            .find(|w| w.chars().all(char::is_numeric) && w.len() == 3)
            .unwrap_or("500")
            .to_string()
        };

        // Check allow list
        for pattern in &settings.allow {
            if glob_match(pattern, &status_code) {
                return true;
            }
        }

        // Check deny list
        for pattern in &settings.deny {
            if glob_match(pattern, &status_code) {
                return false;
            }
        }

        settings.default == "allow"
    }

    fn create_provider_from_config(&self, config: &ModelConfig) -> Result<Box<dyn LLMProvider>> {
        let (provider_name, model_id) = if let Some(ref p) = config.provider {
            (p.to_lowercase(), config.model.clone())
        } else {
            parse_provider_model(&config.model)
        };

        let get_key = |env_var: &Option<String>, default: Option<&String>| -> Result<String> {
            if let Some(var) = env_var {
                env::var(var).map_err(|_| anyhow::anyhow!("Missing env var: {}", var))
            } else if let Some(d) = default {
                Ok(d.clone())
            } else {
                anyhow::bail!("No API key found for {}", provider_name)
            }
        };

        let get_url = |override_url: &Option<String>, default: &String| -> String {
            override_url
                .clone()
                .unwrap_or_else(|| default.clone())
        };

        match provider_name.as_str() {
            "openai" => {
                let default_conf = self.config.providers.openai.as_ref();
                let api_key = get_key(&config.api_key_env, default_conf.map(|c| &c.api_key))?;
                let base_url = get_url(
                    &config.api_base,
                    &default_conf
                        .map(|c| c.base_url.clone())
                        .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                );
                Ok(Box::new(OpenAIProvider::new(
                    &api_key, &base_url, &model_id,
                )?))
            }
            "anthropic" => {
                let default_conf = self.config.providers.anthropic.as_ref();
                let api_key = get_key(&config.api_key_env, default_conf.map(|c| &c.api_key))?;
                let base_url = get_url(
                    &config.api_base,
                    &default_conf
                        .map(|c| c.base_url.clone())
                        .unwrap_or_else(|| "https://api.anthropic.com".to_string()),
                );
                Ok(Box::new(AnthropicProvider::new(
                    &api_key,
                    &base_url,
                    &model_id,
                    self.config.agent.max_tokens,
                )?))
            }
            "ollama" => {
                let default_conf = self.config.providers.ollama.as_ref();
                let endpoint = get_url(
                    &config.api_base,
                    &default_conf
                        .map(|c| c.endpoint.clone())
                        .unwrap_or_else(|| "http://localhost:11434".to_string()),
                );
                Ok(Box::new(OllamaProvider::new(&endpoint, &model_id)?))
            }
            "claude-cli" => {
                let workspace = self.config.workspace_path();
                let cmd = if let Some(c) = &self.config.providers.claude_cli {
                    &c.command
                } else {
                    "claude"
                };
                Ok(Box::new(ClaudeCliProvider::new(
                    cmd, &model_id, workspace,
                )?))
            }
            _ => {
                // Fallback to legacy creation if simple string
                create_provider(&config.model, &self.config)
            }
        }
    }

    pub async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<SmartResponse> {
        let start = Instant::now();
        let mut candidates = vec![self.model_alias.clone()];

        // Add fallbacks from primary
        if let Ok(cfg) = self.resolve_config(&self.model_alias) {
            if let Some(fb) = &cfg.fallback_models {
                candidates.extend(fb.clone());
            }
        }

        let mut last_error = anyhow::anyhow!("No models available");

        for alias in candidates {
            let config = match self.resolve_config(&alias) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to resolve config for {}: {}", alias, e);
                    continue;
                }
            };

            let provider = match self.create_provider_from_config(&config) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to create provider for {}: {}", alias, e);
                    continue;
                }
            };

            match provider.chat(messages, tools).await {
                Ok(response) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let provider_name = if let Some(ref p) = config.provider {
                        p.to_lowercase()
                    } else {
                        let (p, _) = parse_provider_model(&config.model);
                        p
                    };
                    return Ok(SmartResponse {
                        response,
                        used_model: config.model.clone(),
                        provider_name,
                        latency_ms: latency,
                    });
                }
                Err(e) => {
                    warn!("Model {} failed: {}", alias, e);

                    if !self.check_fallback_allowed(&e, &config) {
                        warn!("Fallback denied by settings for error: {}", e);
                        return Err(e);
                    }

                    last_error = e;
                }
            }
        }
        Err(last_error)
    }

    pub async fn summarize(&self, text: &str) -> Result<String> {
        // Just use primary model for summarization for now
        let config = self.resolve_config(&self.model_alias)?;
        let provider = self.create_provider_from_config(&config)?;
        provider.summarize(text).await
    }

    pub async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<StreamResult> {
        let mut candidates = vec![self.model_alias.clone()];

        // Add fallbacks from primary
        if let Ok(cfg) = self.resolve_config(&self.model_alias) {
            if let Some(fb) = &cfg.fallback_models {
                candidates.extend(fb.clone());
            }
        }

        let mut last_error = anyhow::anyhow!("No models available");

        for alias in candidates {
            let config = match self.resolve_config(&alias) {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to resolve config for {}: {}", alias, e);
                    continue;
                }
            };

            let provider = match self.create_provider_from_config(&config) {
                Ok(p) => p,
                Err(e) => {
                    warn!("Failed to create provider for {}: {}", alias, e);
                    continue;
                }
            };

            match provider.chat_stream(messages, tools).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!("Model {} stream failed to start: {}", alias, e);

                    if !self.check_fallback_allowed(&e, &config) {
                        warn!("Fallback denied by settings for error: {}", e);
                        return Err(e);
                    }

                    last_error = e;
                }
            }
        }
        Err(last_error)
    }
}

#[async_trait]
impl LLMProvider for SmartClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<LLMResponse> {
        // Use inherent method
        let resp = self.chat(messages, tools).await?;
        Ok(resp.response)
    }

    async fn summarize(&self, text: &str) -> Result<String> {
        self.summarize(text).await
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<StreamResult> {
        self.chat_stream(messages, tools).await
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.replace("*", ".*");
    let regex = regex::Regex::new(&format!("^{}$", pattern)).unwrap_or_else(|_| regex::Regex::new(".*").unwrap());
    regex.is_match(text)
}

fn parse_provider_model(s: &str) -> (String, String) {
    if let Some((p, m)) = s.split_once('/') {
        (p.to_lowercase(), m.to_string())
    } else if s.starts_with("gpt-") || s.starts_with("o1") {
        ("openai".to_string(), s.to_string())
    } else if s.starts_with("claude-") {
        ("anthropic".to_string(), s.to_string())
    } else {
        ("unknown".to_string(), s.to_string())
    }
}
