use std::collections::HashMap;
use zier_alpha::agent::client::SmartClient;
use zier_alpha::agent::providers::OpenAIProvider;
use zier_alpha::config::{Config, ExtraProviderConfig, ModelConfig};

#[test]
fn test_custom_provider_basic() {
    // Build a config with a custom provider "openrouter"
    let mut config = Config::default();
    config.providers.extra.insert(
        "openrouter".to_string(),
        ExtraProviderConfig {
            api_key: Some("test-key".to_string()),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            r#type: Some("openai".to_string()),
            _other: HashMap::new(),
        },
    );

    // Model config that uses the custom provider
    let model_cfg = ModelConfig {
        provider: Some("openrouter".to_string()),
        model: "stepfun/step-3.5-flash:free".to_string(),
        api_base: None,
        api_key_env: None,
        extend: None,
        timeout: None,
        extra_body: None,
        fallback_models: None,
        fallback_settings: None,
        aliases: None,
        supports_vision: None,
        tokenizer_name: None,
    };

    // Create SmartClient and provider
    let client = SmartClient::new(config, "dummy".to_string());
    let provider = client.create_provider_from_config(&model_cfg).unwrap();

    // Downcast to check configuration
    let any = provider.as_ref() as &dyn std::any::Any;
    let openai_provider = any.downcast_ref::<OpenAIProvider>().unwrap();
    assert_eq!(openai_provider.api_key(), "test-key");
    assert_eq!(
        openai_provider.base_url(),
        "https://openrouter.ai/api/v1"
    );
    assert_eq!(openai_provider.model(), "stepfun/step-3.5-flash:free");
}

#[test]
fn test_custom_provider_with_env_override() {
    // Build a config with a custom provider "together"
    let mut config = Config::default();
    config.providers.extra.insert(
        "together".to_string(),
        ExtraProviderConfig {
            api_key: Some("default-together-key".to_string()),
            base_url: "https://api.together.ai/v1".to_string(),
            r#type: Some("openai".to_string()),
            _other: HashMap::new(),
        },
    );

    // Model config that overrides API key via env var
    let model_cfg = ModelConfig {
        provider: Some("together".to_string()),
        model: "meta-llama/Llama-3-70b-chat-hf".to_string(),
        api_base: Some("https://api.together.ai/v1/custom".to_string()),
        api_key_env: Some("TOGETHER_API_KEY".to_string()),
        extend: None,
        timeout: None,
        extra_body: None,
        fallback_models: None,
        fallback_settings: None,
        aliases: None,
        supports_vision: None,
        tokenizer_name: None,
    };

    // Set the environment variable for the test
    std::env::set_var("TOGETHER_API_KEY", "env-key-123");

    let client = SmartClient::new(config, "dummy".to_string());
    let provider = client.create_provider_from_config(&model_cfg).unwrap();

    let any = provider.as_ref() as &dyn std::any::Any;
    let openai_provider = any.downcast_ref::<OpenAIProvider>().unwrap();
    // Should use the API key from env var, not from config
    assert_eq!(openai_provider.api_key(), "env-key-123");
    // Should use overridden base_url from model config
    assert_eq!(
        openai_provider.base_url(),
        "https://api.together.ai/v1/custom"
    );
}

#[test]
fn test_custom_provider_unknown_falls_back() {
    // Config without the provider in extra
    let config = Config::default();

    // Model config with an unknown provider that's not in extra and not built-in
    let model_cfg = ModelConfig {
        provider: Some("unknown-provider".to_string()),
        model: "some-model".to_string(),
        ..Default::default()
    };

    let client = SmartClient::new(config, "dummy".to_string());
    // Should fall back to create_provider, which will error for unknown provider
    let result = client.create_provider_from_config(&model_cfg);
    assert!(result.is_err());
}
