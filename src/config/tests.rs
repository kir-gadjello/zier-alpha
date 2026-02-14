#[cfg(test)]
mod tests {
    use crate::config::models::{resolve_model_config, ModelConfig};
    use crate::config::{ActiveHours, Config, ExtraProviderConfig, OpenAIConfig};
    use std::collections::HashMap;

    #[test]
    fn test_inheritance_resolution() {
        let mut models = HashMap::new();

        models.insert(
            "base".to_string(),
            ModelConfig {
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
            },
        );

        models.insert(
            "derived".to_string(),
            ModelConfig {
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
            },
        );

        let resolved = resolve_model_config("derived", &models).unwrap();

        assert_eq!(resolved.api_base, Some("https://base.com".to_string())); // Inherited
        assert_eq!(resolved.api_key_env, Some("BASE_KEY".to_string())); // Inherited
        assert_eq!(resolved.model, "derived-model"); // Overridden
        assert_eq!(resolved.timeout, Some(100)); // Inherited
    }

    #[test]
    fn test_cycle_detection() {
        let mut models = HashMap::new();

        models.insert(
            "a".to_string(),
            ModelConfig {
                provider: None,
                api_base: None,
                api_key_env: None,
                model: "a".to_string(),
                extend: Some("b".to_string()),
                timeout: None,
                extra_body: None,
                fallback_models: None,
                fallback_settings: None,
                aliases: None,
                supports_vision: None,
                tokenizer_name: None,
            },
        );

        models.insert(
            "b".to_string(),
            ModelConfig {
                provider: None,
                api_base: None,
                api_key_env: None,
                model: "b".to_string(),
                extend: Some("a".to_string()),
                timeout: None,
                extra_body: None,
                fallback_models: None,
                fallback_settings: None,
                aliases: None,
                supports_vision: None,
                tokenizer_name: None,
            },
        );

        let result = resolve_model_config("a", &models);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Circular dependency"));
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

    #[test]
    fn test_expand_env_vars_extra_providers() {
        use std::env;

        let mut config = Config::default();
        config.providers.extra.insert(
            "custom".to_string(),
            ExtraProviderConfig {
                api_key: Some("${CUSTOM_KEY}".to_string()),
                base_url: "https://custom.com".to_string(),
                r#type: Some("openai".to_string()),
                _other: HashMap::new(),
            },
        );

        env::set_var("CUSTOM_KEY", "expanded-key");
        config.expand_env_vars();

        let expanded = config.providers.extra.get("custom").unwrap();
        assert_eq!(expanded.api_key, Some("expanded-key".to_string()));

        env::remove_var("CUSTOM_KEY");
    }

    #[test]
    fn test_disk_min_free_percent_validation() {
        // Valid: integer within range
        let mut cfg = Config::default();
        cfg.disk.min_free_percent = 5.0;
        assert!(cfg.validate().is_ok());

        // Valid: 0 (disabled threshold)
        cfg.disk.min_free_percent = 0.0;
        assert!(cfg.validate().is_ok());

        // Valid: fractional
        cfg.disk.min_free_percent = 0.1;
        assert!(cfg.validate().is_ok());

        // Valid: 100.0
        cfg.disk.min_free_percent = 100.0;
        assert!(cfg.validate().is_ok());

        // Invalid: negative
        cfg.disk.min_free_percent = -1.0;
        assert!(cfg.validate().is_err());

        // Invalid: >100
        cfg.disk.min_free_percent = 150.0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_disk_min_free_percent_parsing() {
        // Verify that TOML with fractional number parses correctly
        let toml = r#"
[disk]
min_free_percent = 0.1
"#;
        let config: Config = toml::from_str(toml).expect("should parse fractional min_free_percent");
        assert_eq!(config.disk.min_free_percent, 0.1);
    }
}
