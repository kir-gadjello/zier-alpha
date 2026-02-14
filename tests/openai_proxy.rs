use reqwest::Client;
use serde_json::json;
use std::time::Duration;
use tempfile::TempDir;
use zier_alpha::config::Config;
use zier_alpha::server::Server;

#[tokio::test]
async fn test_openai_proxy_chat() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let mut config = Config::default();
    config.memory.workspace = workspace_path.to_string_lossy().to_string();
    config.memory.embedding_provider = "none".to_string();
    config.agent.default_model = "mock/test".to_string();
    config.server.port = 31329;
    config.server.openai_proxy.enabled = true;
    config.server.openai_proxy.port = 37779;
    config.server.openai_proxy.bind = "127.0.0.1".to_string();
    // Prevent disk monitor from entering degraded mode in test environments
    config.disk.min_free_percent = 1;

    let server = Server::new(&config).unwrap();

    tokio::spawn(async move {
        let _: anyhow::Result<()> = server.run().await;
    });

    // Wait for server to start
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let url = "http://127.0.0.1:37779/v1/chat/completions";
    let models_url = "http://127.0.0.1:37779/v1/models";

    // Test list models
    let resp = client.get(models_url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["data"][0]["id"], "mock/test");

    // Test non-streaming with session isolation (User A)
    let resp = client
        .post(url)
        .json(&json!({
            "model": "mock/test",
            "messages": [{"role": "user", "content": "write memory Alice"}],
            "user": "alice",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("Alice"));

    // Test session isolation (User B)
    let resp = client
        .post(url)
        .json(&json!({
            "model": "mock/test",
            "messages": [{"role": "user", "content": "hello"}],
            "user": "bob",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // Bob's session should NOT have Alice's history re-triggered by mock provider logic
    assert_eq!(body["choices"][0]["message"]["content"], "Mock response");

    // Test trace /v
    let resp = client
        .post(url)
        .json(&json!({
            "model": "mock/test",
            "messages": [{"role": "user", "content": "/v write memory Kira"}],
            "stream": false
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let content = body["choices"][0]["message"]["content"].as_str().unwrap();
    assert!(content.contains("Tool Calls Trace"));
    assert!(content.contains("write_file"));
    assert!(content.contains("Kira"));

    // Test streaming
    let mut resp = client
        .post(url)
        .json(&json!({
            "model": "mock/test",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let mut full_content = String::new();
    while let Some(chunk) = resp.chunk().await.unwrap() {
        let chunk_str = String::from_utf8_lossy(&chunk);
        for line in chunk_str.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if data == "[DONE]" {
                    break;
                }
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(content) = json["choices"][0]["delta"]["content"].as_str() {
                        full_content.push_str(content);
                    }
                }
            }
        }
    }
    assert!(full_content.contains("Mock response") || full_content.contains("Kira"));
}
