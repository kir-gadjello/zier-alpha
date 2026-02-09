use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy};
use zier_alpha::config::{Config, WorkdirStrategy, SandboxPolicy};
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;
use tempfile::TempDir;
use std::fs;
use std::io::Write;

#[tokio::test]
async fn test_workdir_overlay_strategy() {
    let workspace_tmp = TempDir::new().unwrap();
    let project_tmp = TempDir::new().unwrap();
    
    let workspace_dir = workspace_tmp.path().to_path_buf();
    let project_dir = project_tmp.path().to_path_buf();
    
    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().to_string();
    config.workdir.strategy = WorkdirStrategy::Overlay;
    config.agent.default_model = "mock/test".to_string();

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test").unwrap();
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 10000,
        reserve_tokens: 100,
    };

    let mut agent = Agent::new_with_project(agent_config, &config, memory, ContextStrategy::Full, project_dir.clone()).await.unwrap();
    agent.new_session().await.unwrap();

    // 1. Write to MEMORY.md (should go to workspace)
    agent.chat("test_tool:write_file|MEMORY.md|Cognitive Content").await.unwrap();
    assert!(workspace_dir.join("MEMORY.md").exists());
    assert!(!project_dir.join("MEMORY.md").exists());
    assert_eq!(fs::read_to_string(workspace_dir.join("MEMORY.md")).unwrap(), "Cognitive Content");

    // 2. Write to main.rs (should go to project_dir)
    agent.chat("test_tool:write_file|main.rs|Project Content").await.unwrap();
    assert!(project_dir.join("main.rs").exists());
    assert!(!workspace_dir.join("main.rs").exists());
    assert_eq!(fs::read_to_string(project_dir.join("main.rs")).unwrap(), "Project Content");
}

#[tokio::test]
async fn test_workdir_mount_strategy() {
    let workspace_tmp = TempDir::new().unwrap();
    let project_tmp = TempDir::new().unwrap();
    
    let workspace_dir = workspace_tmp.path().to_path_buf();
    let project_dir = project_tmp.path().to_path_buf();
    
    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().to_string();
    config.workdir.strategy = WorkdirStrategy::Mount;
    config.agent.default_model = "mock/test".to_string();

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test").unwrap();
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 10000,
        reserve_tokens: 100,
    };

    let mut agent = Agent::new_with_project(agent_config, &config, memory, ContextStrategy::Full, project_dir.clone()).await.unwrap();
    agent.new_session().await.unwrap();

    // 1. Write to MEMORY.md (should go to workspace root)
    agent.chat("test_tool:write_file|MEMORY.md|Cognitive Content").await.unwrap();
    assert!(workspace_dir.join("MEMORY.md").exists());
    assert_eq!(fs::read_to_string(workspace_dir.join("MEMORY.md")).unwrap(), "Cognitive Content");

    // 2. Write to project/main.rs (should go to project_dir/main.rs)
    agent.chat("test_tool:write_file|project/main.rs|Project Content").await.unwrap();
    assert!(project_dir.join("main.rs").exists());
    assert_eq!(fs::read_to_string(project_dir.join("main.rs")).unwrap(), "Project Content");
}

#[tokio::test]
async fn test_deno_tool_routing() {
    let workspace_tmp = TempDir::new().unwrap();
    let project_tmp = TempDir::new().unwrap();
    
    let workspace_dir = workspace_tmp.path().to_path_buf();
    let project_dir = project_tmp.path().to_path_buf();
    
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![workspace_dir.to_string_lossy().to_string(), project_dir.to_string_lossy().to_string()],
        allow_write: vec![workspace_dir.to_string_lossy().to_string(), project_dir.to_string_lossy().to_string()],
    };

    let service = ScriptService::new(policy, workspace_dir.clone(), project_dir.clone(), WorkdirStrategy::Overlay).unwrap();

    let script_content = r#"
        pi.registerTool({
            name: "test_write",
            description: "Write file",
            parameters: {
                type: "object",
                properties: {
                    path: { type: "string" },
                    content: { type: "string" }
                }
            },
            execute: async (id, params) => {
                pi.writeFile(params.path, params.content);
                return "OK";
            }
        });
    "#;

    let mut script_file = tempfile::NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();
    service.load_script(script_file.path().to_str().unwrap()).await.unwrap();

    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().to_string();
    config.workdir.strategy = WorkdirStrategy::Overlay;
    config.agent.default_model = "mock/test".to_string();

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test").unwrap();
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 10000,
        reserve_tokens: 100,
    };

    let mut agent = Agent::new_with_project(agent_config, &config, memory, ContextStrategy::Full, project_dir.clone()).await.unwrap();
    agent.new_session().await.unwrap();
    
    // Inject the script tool into the agent
    let tools = service.get_tools().await.unwrap();
    let script_tool = zier_alpha::agent::ScriptTool::new(tools[0].clone(), service);
    agent.set_tools(vec![Box::new(script_tool)]);

    // 1. Write to MEMORY.md via Deno (should go to workspace)
    agent.chat("test_tool:test_write|MEMORY.md|Deno Cognitive Content").await.unwrap();
    assert!(workspace_dir.join("MEMORY.md").exists());
    assert_eq!(fs::read_to_string(workspace_dir.join("MEMORY.md")).unwrap(), "Deno Cognitive Content");

    // 2. Write to app.js via Deno (should go to project_dir)
    agent.chat("test_tool:test_write|app.js|Deno Project Content").await.unwrap();
    assert!(project_dir.join("app.js").exists());
    assert_eq!(fs::read_to_string(project_dir.join("app.js")).unwrap(), "Deno Project Content");
}
