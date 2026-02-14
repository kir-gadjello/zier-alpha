use std::fs;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use zier_alpha::config::{AgentConfig, Config, MemoryConfig, ServerConfig};
use zier_alpha::ingress::controller::ingress_loop;
use zier_alpha::ingress::{IngressBus, IngressMessage, TrustLevel};
use zier_alpha::prompts::PromptRegistry;

#[tokio::test]
async fn test_architecture_concurrency() {
    // 1. Setup
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();
    let artifacts_path = workspace_path.join("artifacts");

    let memory_config = MemoryConfig {
        workspace: workspace_path.to_string_lossy().to_string(),
        embedding_provider: "none".to_string(),
        ..Default::default()
    };

    let config = Config {
        memory: memory_config,
        agent: AgentConfig {
            default_model: "mock/test".to_string(),
            ..Default::default()
        },
        server: ServerConfig::default(),
        workdir: zier_alpha::config::WorkdirConfig {
            strategy: zier_alpha::config::WorkdirStrategy::Overlay,
            ..Default::default()
        },
        ..Default::default()
    };

    let bus = Arc::new(IngressBus::new(100));
    let prompts = Arc::new(PromptRegistry::new());

    // Spawn Ingress Loop
    let receiver = bus.receiver();
    let config_clone = config.clone();
    let prompts_clone = prompts.clone();

    let script_service = zier_alpha::scripting::ScriptService::new(
        zier_alpha::config::SandboxPolicy::default(),
        workspace_path.clone(),
        workspace_path.clone(),
        zier_alpha::config::WorkdirStrategy::Overlay,
        Some(bus.clone()),
        None,
    )
    .unwrap();

    tokio::spawn(async move {
        ingress_loop(
            receiver,
            config_clone,
            "main".to_string(),
            prompts_clone,
            script_service,
            vec![],
        )
        .await;
    });

    // 2. Send multiple concurrent messages
    let count = 10;
    for i in 0..count {
        let msg = IngressMessage::new(
            format!("test_source_{}", i),
            format!("Hello {}", i),
            TrustLevel::OwnerCommand,
        );
        bus.push(msg).await.unwrap();
    }

    // 3. Wait for processing
    let mut success = false;
    for _ in 0..50 {
        // Wait up to 5s
        tokio::time::sleep(Duration::from_millis(100)).await;
        if artifacts_path.exists() {
            if let Ok(entries) = fs::read_dir(&artifacts_path) {
                let count_artifacts = entries.count();
                if count_artifacts == count {
                    success = true;
                    break;
                }
            }
        }
    }

    assert!(success, "Not all artifacts were created in time");
}
