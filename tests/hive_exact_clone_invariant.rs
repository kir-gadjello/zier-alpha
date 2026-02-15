use anyhow::Result;
use std::path::PathBuf;
use tempfile::TempDir;

use zier_alpha::agent::Agent;
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;

/// Recursively copy a directory (not used here but kept for potential future use)
#[allow(dead_code)]
fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn test_hive_exact_clone_system_prompt_prefix_invariant() -> Result<()> {
    // This test verifies that when a Hive child agent declares `model: "."` and `tools: "."`,
    // the resulting system prompt prefix (excluding the memory context) is byte-identical
    // between parent and child, except for the "Current Time" section and status lines which
    // are process-specific.
    //
    // We simulate Hive's child creation mechanism by:
    //  - Creating a parent Agent
    //  - Creating a child Agent with the same config and workspace
    //  - Overriding the child's tool list to exactly match parent's tools
    //  - Comparing their system prompts after session creation.

    // Disable disk monitoring to avoid flakiness in CI environments
    std::env::set_var("ZIER_ALPHA_DISABLE_DISK_MONITOR", "1");

    let temp_dir = TempDir::new()?;
    let root = temp_dir.path().to_path_buf();

    // Setup workspace and HOME
    let workspace_dir = root.join("workspace");
    std::fs::create_dir(&workspace_dir)?;
    let home_dir = root.join("home");
    std::fs::create_dir_all(&home_dir)?;
    let dot_zier = home_dir.join(".zier-alpha");
    std::fs::create_dir_all(&dot_zier)?;

    // Minimal config with mock provider
    let config_content = format!(
        r#"
[agent]
default_model = "mock/test"

[memory]
workspace = "{}"
"#,
        workspace_dir.display()
    );
    std::fs::write(dot_zier.join("config.toml"), config_content)?;

    // Set HOME to temp dir so Config::load picks it up
    std::env::set_var("HOME", &home_dir);

    // Load config
    let config = Config::load()?;

    // Create separate memory managers for parent and child
    let parent_memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "main")?;
    let child_memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "main")?;

    // Build parent agent
    let mut parent = Agent::new_with_project(
        zier_alpha::agent::AgentConfig {
            model: config.agent.default_model.clone(),
            context_window: config.agent.context_window,
            reserve_tokens: config.agent.reserve_tokens,
        },
        &config,
        parent_memory,
        zier_alpha::agent::ContextStrategy::Full,
        root.clone(),
        "test",
    )
    .await?;

    // Parent session
    parent.new_session().await?;

    // Get parent's full system prompt (the context string)
    let parent_system = parent
        .system_prompt()
        .await
        .expect("Parent system prompt should exist");

    // Build child agent with same base config
    let mut child = Agent::new_with_project(
        zier_alpha::agent::AgentConfig {
            model: config.agent.default_model.clone(),
            context_window: config.agent.context_window,
            reserve_tokens: config.agent.reserve_tokens,
        },
        &config,
        child_memory,
        zier_alpha::agent::ContextStrategy::Full,
        root,
        "test",
    )
    .await?;

    // Override child tools to exactly match parent's tools (simulating tools: ".")
    let parent_tools = parent.tools();
    child.set_tools(parent_tools.iter().cloned().collect());

    // Child session
    child.new_session().await?;

    // Get child's full system prompt
    let child_system = child
        .system_prompt()
        .await
        .expect("Child system prompt should exist");

    // Both prompts have the structure:
    //   <system prompt base>
    //   \n\n---\n\n# Workspace Context\n\n<memory context>
    // We only care about the base part (before the delimiter)
    let delimiter = "\n\n---\n\n# Workspace Context\n\n";
    let parent_base = parent_system
        .split(delimiter)
        .next()
        .unwrap_or(&parent_system);
    let child_base = child_system
        .split(delimiter)
        .next()
        .unwrap_or(&child_system);

    // Normalize by removing the "Current Time" section entirely, because timestamps differ.
    // Also strip any status lines which may differ between agents.
    fn normalize(base: &str) -> String {
        let mut result = String::new();
        let mut lines = base.lines();
        let mut skip_next = false;
        while let Some(line) = lines.next() {
            if line == "## Current Time" {
                // Skip this line and the next line (the time)
                skip_next = true;
                continue;
            }
            if skip_next {
                skip_next = false;
                continue;
            }
            // Skip status lines (they appear as "- ..." lines after "## Status" if present)
            // We don't have a Status section in current prompt, but be robust.
            // For now, we only remove time.
            result.push_str(line);
            result.push('\n');
        }
        result
    }

    let parent_norm = normalize(parent_base);
    let child_norm = normalize(child_base);

    // The invariant: after normalizing time, the base prompts should be byte-identical.
    assert_eq!(
        parent_norm,
        child_norm,
        "System prompt base (with time stripped) differs between parent and child.\n\
         Parent normalized length: {}\n\
         Child normalized length: {}",
        parent_norm.len(),
        child_norm.len()
    );

    Ok(())
}
