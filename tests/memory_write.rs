use std::fs;
use tempfile::TempDir;
use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy};
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;

#[tokio::test]
async fn test_memory_write_integration() {
    // 1. Setup
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let mut config = Config::default();
    config.memory.workspace = workspace_path.to_string_lossy().to_string();
    config.agent.default_model = "mock/test".to_string();

    let memory =
        MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent").unwrap();

    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 100000,
        reserve_tokens: 1000,
    };

    let mut agent = Agent::new(agent_config, &config, memory, ContextStrategy::Full, "test")
        .await
        .unwrap();
    agent.new_session().await.unwrap();

    // 2. Chat to write memory
    let response = agent
        .chat("test: write memory - my name is Kira")
        .await
        .unwrap();

    assert!(response.contains("saved your name as Kira"));

    // 3. Verify file was written
    let memory_file = workspace_path.join("MEMORY.md");
    assert!(memory_file.exists());

    let content = fs::read_to_string(memory_file).unwrap();
    assert!(content.contains("Name: Kira"));
}
