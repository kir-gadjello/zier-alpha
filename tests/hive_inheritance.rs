/// Integration tests for Hive agent config inheritance feature.
use anyhow::Result;
use regex::Regex;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

/// Recursively copy directory
fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<()> {
    if !src.is_dir() {
        return Ok(());
    }
    fs::create_dir_all(dst)?;
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

/// Execute `zier ask` with given task and return (stdout, stderr)
fn run_zier_ask(root: &tempfile::TempDir, task: &str) -> Result<(String, String)> {
    let bin_path = env!("CARGO_BIN_EXE_zier-alpha");
    let bin_dir = root.path().join("bin");
    let home_dir = root.path().to_path_buf();
    let workspace_dir = root.path().join("workspace");

    let output = Command::new(bin_path)
        .arg("ask")
        .arg(task)
        .env("HOME", &home_dir)
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("RUST_LOG", "info")
        .current_dir(root.path())
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        anyhow::bail!("zier ask failed: {} | stderr: {}", stdout, stderr);
    }
    Ok((stdout, stderr))
}

#[test]
fn test_hive_inheritance_behavior() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path();

    // Environment setup
    let workspace_dir = root.join("workspace");
    fs::create_dir(&workspace_dir)?;
    let agents_dir = root.join("agents");
    fs::create_dir(&agents_dir)?;
    let ext_dir = root.join("extensions").join("hive");
    fs::create_dir_all(&ext_dir)?;
    let source_ext = PathBuf::from("extensions/hive");
    if source_ext.exists() {
        copy_dir_recursive(&source_ext, &ext_dir)?;
    } else {
        eprintln!(
            "Hive extension source not found at {}",
            source_ext.display()
        );
        return Ok(());
    }

    // Config
    let config_path = root.join("config.toml");
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
    fs::write(&config_path, config_content)?;

    // Parent agent
    fs::write(
        agents_dir.join("parent.md"),
        r#"---
description: "Parent agent"
model: "mock/gpt-4o"
tools: ["hive_fork_subagent", "bash", "read_file"]
context_mode: "fresh"
---
You are Parent. You can delegate to children using hive_fork_subagent.
"#,
    )?;

    // Child agent: tools ".no_delegate", model "."
    fs::write(
        agents_dir.join("child.md"),
        r#"---
description: "Child agent"
model: "."
tools: ".no_delegate"
context_mode: "fresh"
---
You are Child. You inherit parent's model and tools except hive_fork_subagent.
"#,
    )?;

    // Child2 agent: tools "."
    fs::write(
        agents_dir.join("child2.md"),
        r#"---
description: "Child2 agent"
model: "."
tools: "."
context_mode: "fresh"
---
You are Child2. You inherit everything.
"#,
    )?;

    // Bin setup
    let bin_path = env!("CARGO_BIN_EXE_zier-alpha");
    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir)?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(bin_path, bin_dir.join("zier"))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&bin_path, bin_dir.join("zier.exe"))?;
    let home_dir = root.to_path_buf();
    let dot_zier = home_dir.join(".zier-alpha");
    fs::create_dir_all(&dot_zier)?;
    fs::rename(&config_path, dot_zier.join("config.toml"))?;

    // Test 1: .no_delegate filters out hive_fork_subagent, model inherited
    let task1 =
        r#"test_tool_json:hive_fork_subagent|{"agent_name": "child", "task": "list files using bash"}"#;
    let (stdout1, stderr1) = run_zier_ask(&temp_dir, task1)?;
    // Combine both output streams for log search
    let combined1 = format!("{}\n{}", stdout1, stderr1);
    let ansi = Regex::new(r"\x1B\[[0-9;]*[A-Za-z]").unwrap();
    let clean1 = ansi.replace_all(&combined1, "").to_string();

    // Ensure delegation succeeded
    assert!(
        stdout1.contains("Mock response"),
        "Parent didn't get expected response: {}",
        stdout1
    );

    // Verify child spawn details: parent model inherited and tool count = builtins count (7)
    assert!(
        clean1.contains("[Hive] Spawning:"),
        "Missing Hive spawn log in combined output:\n{}",
        combined1
    );
    assert!(
        clean1.contains("--model mock/gpt-4o"),
        "Child did not inherit parent model:\n{}",
        clean1
    );
    // Parent has builtins (7) + hive_delegate = 8 tools. With .no_delegate, child gets 7.
    assert!(
        clean1.contains("(tools: 7)"),
        "Expected child tools=7 after .no_delegate, got:\n{}",
        clean1
    );

    // Test 2: tools "." inherits all parent tools (including hive_fork_subagent)
    let task2 = r#"test_tool_json:hive_fork_subagent|{"agent_name": "child2", "task": "delegate to grandchild: ping"}"#;
    let (stdout2, stderr2) = run_zier_ask(&temp_dir, task2)?;
    let combined2 = format!("{}\n{}", stdout2, stderr2);
    let clean2 = ansi.replace_all(&combined2, "").to_string();

    assert!(
        stdout2.contains("Mock response"),
        "Parent didn't get response for child2: {}",
        stdout2
    );
    assert!(
        clean2.contains("[Hive] Spawning:"),
        "Missing Hive spawn log for child2:\n{}",
        combined2
    );
    // child2 inherits full parent toolset (8)
    assert!(
        clean2.contains("(tools: 8)"),
        "Expected child2 tools=8 via '.' inheritance, got:\n{}",
        clean2
    );

    Ok(())
}
