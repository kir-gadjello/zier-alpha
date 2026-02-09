use serde::Deserialize;
use std::collections::HashMap;
use anyhow::Result;

#[derive(Deserialize, Clone, Debug)]
pub struct ModelConfig {
    pub api_base: Option<String>,
    pub api_key_env: Option<String>, // e.g., "OPENROUTER_API_KEY"
    pub model: String, // The actual wire name (e.g., "openai/gpt-4o")
    pub extend: Option<String>, // Parent key
    pub timeout: Option<u64>,
    pub extra_body: Option<serde_json::Value>,
    pub fallback_models: Option<Vec<String>>,
    pub fallback_settings: Option<FallbackSettings>,
    pub aliases: Option<Vec<String>>,
    pub supports_vision: Option<bool>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FallbackSettings {
    pub default: String, // "deny" or "allow"
    pub allow: Vec<String>, // ["429", "5*"]
    pub deny: Vec<String>,
}

pub fn resolve_model_config(
    model_key: &str,
    models: &HashMap<String, ModelConfig>,
) -> Result<ModelConfig> {
    let mut current_key = model_key.to_string();
    let mut visited = vec![current_key.clone()];
    let mut config_chain = Vec::new();

    // 1. Build inheritance chain
    loop {
        if let Some(config) = models.get(&current_key) {
            config_chain.push(config.clone());
            if let Some(parent) = &config.extend {
                if visited.contains(parent) {
                    anyhow::bail!("Circular dependency detected in model config: {:?}", visited);
                }
                current_key = parent.clone();
                visited.push(current_key.clone());
            } else {
                break;
            }
        } else {
            // If the key itself is not in the map, it might be a direct model name or alias not defined in [models]
            // For now, we assume if we are calling resolve, it MUST be in the map.
            // If strictly using simple strings in other parts of config, they won't use this resolver.
            anyhow::bail!("Model config not found: {}", current_key);
        }
    }

    // 2. Merge configs (child overrides parent)
    // We iterate in reverse order (parent -> child)
    config_chain.reverse();

    let mut final_config = config_chain[0].clone();

    for child in config_chain.iter().skip(1) {
        if let Some(v) = &child.api_base { final_config.api_base = Some(v.clone()); }
        if let Some(v) = &child.api_key_env { final_config.api_key_env = Some(v.clone()); }
        final_config.model = child.model.clone(); // Always override model name
        // extend is irrelevant in final config
        if let Some(v) = child.timeout { final_config.timeout = Some(v); }
        if let Some(v) = &child.extra_body { final_config.extra_body = Some(v.clone()); }
        if let Some(v) = &child.fallback_models { final_config.fallback_models = Some(v.clone()); }
        if let Some(v) = &child.fallback_settings { final_config.fallback_settings = Some(v.clone()); }
        if let Some(v) = &child.aliases { final_config.aliases = Some(v.clone()); }
        if let Some(v) = child.supports_vision { final_config.supports_vision = Some(v); }
    }

    Ok(final_config)
}
