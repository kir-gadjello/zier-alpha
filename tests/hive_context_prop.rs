use std::sync::Arc;
use tempfile::NamedTempFile;
use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy};
use zier_alpha::config::{Config, SandboxPolicy, WorkdirStrategy};
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_hive_context_propagation() {
    // 1. Setup
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // Create a dummy config
    let config = Config::default();

    // Initialize MemoryManager (requires creating some dirs)
    std::fs::create_dir_all(workspace_path.join("memory")).unwrap();
    let memory = MemoryManager::new_with_full_config(
        &config.memory,
        Some(&config),
        "test-agent-hive"
    ).unwrap();

    let agent_config = AgentConfig {
        model: "initial-model".to_string(),
        context_window: 4096,
        reserve_tokens: 100,
    };

    // 2. Create ScriptService
    let policy = SandboxPolicy::default();
    let script_service = ScriptService::new(
        policy,
        workspace_path.clone(),
        workspace_path.clone(),
        WorkdirStrategy::Overlay,
        None,
        None,
        None,
        "test-agent-hive".to_string(),
    ).unwrap();

    // 3. Create Agent
    let mut agent = Agent::new_with_project(
        agent_config,
        &config,
        memory,
        ContextStrategy::Stateless,
        workspace_path.clone(),
        "test-agent-hive",
    ).await.unwrap();

    // 4. Inject ScriptService
    agent.set_script_service(script_service.clone());

    // 5. Verify initial state (should be updated by set_tools inside new_with_project?
    //    No, set_script_service is called AFTER new. So we need to trigger an update.)

    // Trigger update via set_model
    agent.set_model("updated-model").unwrap();

    // Give async task time to propagate
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 6. Verify ScriptService state via a script
    // We create a script that calls `zier.getParentContext()` and returns it.
    let script_content = r#"
        pi.registerTool({
            name: "get_context",
            description: "Get parent context",
            parameters: { type: "object", properties: {} },
            execute: async () => {
                const ctx = zier.getParentContext();
                return JSON.stringify(ctx);
            }
        });
    "#;

    let mut script_file = NamedTempFile::new().unwrap();
    use std::io::Write;
    script_file.write_all(script_content.as_bytes()).unwrap();

    script_service.load_script(script_file.path().to_str().unwrap()).await.unwrap();

    let result = script_service.execute_tool("get_context", "{}").await.unwrap();

    // Parse result
    // The tool returns JSON string of the context object
    let ctx: serde_json::Value = serde_json::from_str(&result).unwrap();

    println!("Context result: {}", ctx);

    assert_eq!(ctx["model"], "updated-model");
    assert_eq!(ctx["agentId"], "test-agent-hive");

    // 7. Verify Tools propagation
    // agent.tools() should contain default tools
    let tools = agent.tools();
    assert!(!tools.is_empty());

    // Trigger update via set_tools (effectively same tools, but triggers propagation)
    agent.set_tools(tools.to_vec());

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let result_tools = script_service.execute_tool("get_context", "{}").await.unwrap();
    let ctx_tools: serde_json::Value = serde_json::from_str(&result_tools).unwrap();

    let tool_list = ctx_tools["tools"].as_array().expect("Tools should be an array");
    assert!(!tool_list.is_empty());

    // Check for "bash" tool which is default
    let has_bash = tool_list.iter().any(|t| t.as_str() == Some("bash"));
    assert!(has_bash, "Parent context should include bash tool");
}
