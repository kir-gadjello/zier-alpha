use tempfile::TempDir;
use zier_alpha::agent::providers::Role;
use zier_alpha::agent::Agent;
use zier_alpha::agent::AgentConfig;
use zier_alpha::agent::ContextStrategy;
use zier_alpha::config::{Config, SandboxPolicy, WorkdirStrategy};
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_system_prompt_generator_integration() {
    // Setup temp workspace
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    // Create Config
    let mut config = Config::default();
    config.memory.workspace = workspace.to_string_lossy().into_owned();
    // We'll set system_prompt_script after writing script

    // Create MemoryManager
    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent")
        .expect("Failed to create memory manager");

    // Create ScriptService with permissive sandbox for test
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![workspace.to_str().unwrap().to_string()],
        allow_write: vec![workspace.to_str().unwrap().to_string()],
        allow_env: false,
        enable_os_sandbox: false,
    };
    let script_service = ScriptService::new(
        policy,
        workspace.clone(),
        workspace.clone(),
        WorkdirStrategy::Overlay,
        None,
        None,
        None,
        "test-agent".to_string(),
    )
    .expect("Failed to create script service");

    // Write generator script to temp file
    let script_content = r#"
        globalThis.generateSystemPrompt = (ctx) => {
            return `CUSTOM_PROMPT: model=${ctx.model}, tools=${ctx.tool_names.join(",")}`;
        };
    "#;
    let script_path = workspace.join("generator.js");
    std::fs::write(&script_path, script_content).unwrap();

    // Update config to use this script
    config.agent.system_prompt_script = Some(script_path.to_str().unwrap().to_string());

    // Create Agent with mock model to avoid real LLM calls
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 128000,
        reserve_tokens: 8000,
    };
    let mut agent = Agent::new_with_project(
        agent_config,
        &config,
        memory,
        ContextStrategy::Stateless,
        workspace,
        "test-agent",
    )
    .await
    .expect("Failed to create agent");

    // Attach script_service
    agent.set_script_service(script_service);

    // Start new session
    agent.new_session().await.expect("new_session failed");

    // Retrieve session system message
    let messages = agent.session_messages().await;
    let system_msg = messages
        .iter()
        .find(|m| m.role == Role::System)
        .expect("System message not found");

    assert!(
        system_msg
            .content
            .contains("CUSTOM_PROMPT: model=mock/test"),
        "System prompt did not include custom marker. Full content: {}",
        system_msg.content
    );
    assert!(
        system_msg.content.contains("tools="),
        "System prompt did not include tools. Full content: {}",
        system_msg.content
    );
}
