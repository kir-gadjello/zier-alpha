use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use std::sync::Arc;
use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy, ScriptTool};
use zier_alpha::config::{Config, SandboxPolicy};
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_context_visibility() -> Result<()> {
    eprintln!("TEST: starting");
    // Setup temporary environment
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path().to_path_buf();
    eprintln!("TEST: temp dir created");

    // Directories
    let workspace_dir = root.join("workspace");
    fs::create_dir(&workspace_dir)?;
    let agents_dir = root.join("agents");
    fs::create_dir(&agents_dir)?;
    eprintln!("TEST: dirs created");

    // Prepare HOME with config and extension
    let home_dir = root.join("home");
    fs::create_dir(&home_dir)?;
    let dot_zier = home_dir.join(".zier-alpha");
    fs::create_dir_all(&dot_zier)?;
    eprintln!("TEST: home dir created");

    // Copy Hive extension to home extensions directory
    let source_ext = PathBuf::from("extensions/hive");
    let ext_dst = dot_zier.join("extensions").join("hive");
    if source_ext.exists() {
        copy_dir_recursive(&source_ext, &ext_dst)?;
    } else {
        eprintln!(
            "Hive extension source not found at {}",
            source_ext.display()
        );
        // Skip test if extension not present
        return Ok(());
    }
    eprintln!("TEST: Hive extension copied");

    // Create a dummy agent (echo) for Hive to load
    let agent_path = agents_dir.join("echo.md");
    fs::write(
        &agent_path,
        r#"---
description: "Echo bot"
model: "mock/gpt-4o"
tools: ["hive_fork_subagent"]
context_mode: "fresh"
---
You are EchoBot.
"#,
    )?;
    eprintln!("TEST: echo agent created");

    // Create config.toml
    let config_content = format!(
        r#"
[agent]
default_model = "mock/gpt-4o"

[extensions.hive]
enabled = true
agents_dir = "agents"

[memory]
workspace = "{}"
"#,
        workspace_dir.display()
    );
    fs::write(dot_zier.join("config.toml"), config_content)?;
    eprintln!("TEST: config written");

    // Set environment variables for the test process
    std::env::set_var("HOME", &home_dir);
    std::env::set_var("ZIER_ALPHA_WORKSPACE", &workspace_dir);
    eprintln!("TEST: env set");

    // Load configuration
    let config = Config::load()?;
    eprintln!("TEST: config loaded");

    // Create MemoryManager
    let agent_id = "test";
    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), agent_id)?;
    eprintln!("TEST: memory manager created");

    let agent_config = AgentConfig {
        model: config.agent.default_model.clone(),
        context_window: config.agent.context_window,
        reserve_tokens: config.agent.reserve_tokens,
    };

    eprintln!("TEST: about to create agent");
    // Create Agent (Stateless as used by ask)
    let mut agent = Agent::new_with_project(
        agent_config,
        &config,
        memory,
        ContextStrategy::Stateless,
        root.clone(),
        "test",
    )
    .await?;
    eprintln!("TEST: agent created");

    // Load Hive extension (before session creation)
    if let Some(ref hive_config) = config.extensions.hive {
        if hive_config.enabled {
            // Find hive extension path (we placed it in home/.zier-alpha/extensions/hive)
            let hive_path = dot_zier.join("extensions").join("hive").join("main.js");
            if hive_path.exists() {
                eprintln!("TEST: loading Hive script from {:?}", hive_path);
                // Build a permissive policy for the test
                let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
                let policy = SandboxPolicy {
                    allow_env: true,
                    allow_read: vec![
                        temp_dir.clone(),
                        workspace_dir.to_string_lossy().to_string(),
                        root.to_string_lossy().to_string(),
                    ],
                    allow_write: vec![
                        temp_dir,
                        workspace_dir.to_string_lossy().to_string(),
                        root.to_string_lossy().to_string(),
                    ],
                    ..Default::default()
                };

                let service = ScriptService::new(
                    policy,
                    config.workspace_path(),
                    root.clone(),
                    config.workdir.strategy.clone(),
                    None,
                    None,
                    None,
                    "test".to_string(),
                )?;
                eprintln!("TEST: ScriptService created, loading script");
                service.load_script(hive_path.to_str().unwrap()).await?;
                eprintln!("TEST: script loaded, getting tools");
                let tools = service.get_tools().await?;
                eprintln!("TEST: got {} tools", tools.len());
                let mut current_tools = agent.tools().to_vec();
                for tool_def in tools {
                    current_tools.push(Arc::new(ScriptTool::new(tool_def, service.clone())));
                }
                agent.set_tools(current_tools);
                eprintln!("TEST: tools set on agent");
            } else {
                return Err(anyhow::anyhow!(
                    "Hive extension not found at {:?}",
                    hive_path
                ));
            }
        }
    } else {
        eprintln!("TEST: Hive not enabled in config");
    }

    eprintln!("TEST: about to create session");
    // Create session (after Hive loading)
    agent.new_session().await?;
    eprintln!("TEST: session created");

    // Assert that hive_fork_subagent tool is registered
    let tool_names: Vec<String> = agent.tools().iter().map(|t| t.name().to_string()).collect();
    eprintln!("TEST: tools: {:?}", tool_names);
    assert!(
        tool_names.contains(&"hive_fork_subagent".to_string()),
        "hive_fork_subagent tool not found in agent tools. Available tools: {:?}",
        tool_names
    );

    // Assert that system prompt includes hive_fork_subagent
    let system_ctx = agent
        .system_prompt()
        .await
        .ok_or_else(|| anyhow::anyhow!("System context not set"))?;
    eprintln!("TEST: system prompt length: {}", system_ctx.len());
    assert!(
        system_ctx.contains("hive_fork_subagent"),
        "System prompt does not mention hive_fork_subagent. System prompt: {}",
        system_ctx
    );

    eprintln!("TEST: all assertions passed");
    Ok(())
}

// Helper: copy directory recursively
fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
