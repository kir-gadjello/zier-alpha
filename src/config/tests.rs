#[cfg(test)]
mod tests {
    use crate::config::models::{resolve_model_config, ModelConfig};
    use crate::config::{Config, HeartbeatConfig, ActiveHours, ProvidersConfig, OpenAIConfig};
    use std::collections::HashMap;

    #[test]
    fn test_inheritance_resolution() {
        let mut models = HashMap::new();

        models.insert("base".to_string(), ModelConfig {
            provider: None,
            api_base: Some("https://base.com".to_string()),
            api_key_env: Some("BASE_KEY".to_string()),
            model: "base-model".to_string(),
            extend: None,
            timeout: Some(100),
            extra_body: None,
            fallback_models: None,
            fallback_settings: None,
            aliases: None,
            supports_vision: None,
            tokenizer_name: None,
        });

        models.insert("derived".to_string(), ModelConfig {
            provider: None,
            api_base: None,
            api_key_env: None,
            model: "derived-model".to_string(),
            extend: Some("base".to_string()),
            timeout: None,
            extra_body: None,
            fallback_models: None,
            fallback_settings: None,
            aliases: None,
            supports_vision: None,
            tokenizer_name: None,
        });

        let resolved = resolve_model_config("derived", &models).unwrap();

        assert_eq!(resolved.api_base, Some("https://base.com".to_string())); // Inherited
        assert_eq!(resolved.api_key_env, Some("BASE_KEY".to_string())); // Inherited
        assert_eq!(resolved.model, "derived-model"); // Overridden
        assert_eq!(resolved.timeout, Some(100)); // Inherited
    }

    #[test]
    fn test_cycle_detection() {
        let mut models = HashMap::new();

        models.insert("a".to_string(), ModelConfig {
            provider: None, api_base: None, api_key_env: None, model: "a".to_string(),
            extend: Some("b".to_string()), timeout: None, extra_body: None,
            fallback_models: None, fallback_settings: None, aliases: None, supports_vision: None,
            tokenizer_name: None,
        });

        models.insert("b".to_string(), ModelConfig {
            provider: None, api_base: None, api_key_env: None, model: "b".to_string(),
            extend: Some("a".to_string()), timeout: None, extra_body: None,
            fallback_models: None, fallback_settings: None, aliases: None, supports_vision: None,
            tokenizer_name: None,
        });

        let result = resolve_model_config("a", &models);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Circular dependency"));
    }

    #[test]
    fn test_config_validation_heartbeat() {
        let mut config = Config::default();
        config.heartbeat.enabled = true;
        config.heartbeat.interval = "".to_string(); // Invalid

        assert!(config.validate().is_err());

        config.heartbeat.interval = "30m".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_active_hours() {
        let mut config = Config::default();
        config.heartbeat.active_hours = Some(ActiveHours {
            start: "9:00".to_string(), // Invalid (needs 09:00)
            end: "17:00".to_string(),
        });

        assert!(config.validate().is_err());

        config.heartbeat.active_hours = Some(ActiveHours {
            start: "09:00".to_string(),
            end: "17:00".to_string(),
        });
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_api_keys() {
        let mut config = Config::default();
        config.providers.openai = Some(OpenAIConfig {
            api_key: "".to_string(), // Missing key
            base_url: "https://api.openai.com/v1".to_string(),
        });

        assert!(config.validate().is_err());

        config.providers.openai = Some(OpenAIConfig {
            api_key: "sk-test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
        });
        assert!(config.validate().is_ok());
    }
}
