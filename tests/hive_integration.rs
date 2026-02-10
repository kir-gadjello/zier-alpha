use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

#[test]
fn test_hive_integration() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path();

    // 1. Setup Environment
    let workspace_dir = root.join("workspace");
    fs::create_dir(&workspace_dir)?;

    let agents_dir = root.join("agents");
    fs::create_dir(&agents_dir)?;

    // Copy hive extension to temp dir
    let ext_dir = root.join("extensions").join("hive");
    fs::create_dir_all(&ext_dir)?;

    // We assume the test is running in repo root
    let source_ext = PathBuf::from("extensions/hive");
    if source_ext.exists() {
        copy_dir_recursive(&source_ext, &ext_dir)?;
    } else {
        eprintln!("Hive extension source not found at {}", source_ext.display());
        // In some CI envs, cwd might be different. Try finding it.
        // But for this environment, it should be present.
        return Ok(());
    }

    // Create config
    let config_path = root.join("config.toml");
    let config_content = format!(r#"
[agent]
default_model = "mock/gpt-4o"

[extensions.hive]
enabled = true
agents_dir = "agents"

[memory]
workspace = "{}"

[providers.mock]
"#, workspace_dir.display());
    fs::write(&config_path, config_content)?;

    // Create a test agent in the "agents" dir (scanned by registry)
    let agent_path = agents_dir.join("echo.md");
    fs::write(&agent_path, r#"---
description: "Echo bot"
model: "mock/gpt-4o"
tools: ["hive_delegate"]
context_mode: "fresh"
---
You are EchoBot.
"#)?;

    // 2. Run Test: hive_fresh_basic
    // Trigger delegation via mock tool call
    let bin_path = env!("CARGO_BIN_EXE_zier-alpha");

    // Create 'zier' symlink in the same dir as the binary (target/debug/...)
    // Note: We might not have write permission there in some environments, or parallel tests might conflict.
    // Better to create a symlink in our temp dir and add temp dir to PATH.
    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir)?;

    #[cfg(unix)]
    std::os::unix::fs::symlink(&bin_path, bin_dir.join("zier"))?;
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&bin_path, bin_dir.join("zier.exe"))?;

    // Set HOME to temp dir so config is loaded from there (.zier-alpha/config.toml)
    let home_dir = root.to_path_buf();

    // We need to move config.toml to .zier-alpha/config.toml in HOME
    let dot_zier = home_dir.join(".zier-alpha");
    fs::create_dir_all(&dot_zier)?;
    fs::rename(&config_path, dot_zier.join("config.toml"))?;

    let output = Command::new(bin_path)
        .arg("ask")
        .arg("test_tool_json:hive_delegate|{\"agent_name\": \"echo\", \"task\": \"hello\"}")
        .env("HOME", &home_dir) // Force Config::load to use our config
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        // Add our temp bin dir to PATH so 'zier' is found
        .env("PATH", format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default()))
        .current_dir(root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        panic!("zier ask failed with status: {:?}", output.status);
    }

    assert!(stdout.contains("Mock response"), "Output did not contain expected mock response");

    // 3. Test: hive_depth_limit
    let output_depth = Command::new(&bin_path)
        .arg("ask")
        .arg("test_tool_json:hive_delegate|{\"agent_name\": \"echo\", \"task\": \"hello\"}")
        .env("HOME", &home_dir)
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        .env("PATH", format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default()))
        .env("ZIER_HIVE_DEPTH", "3")
        .current_dir(root)
        .output()?;

    let stdout_depth = String::from_utf8_lossy(&output_depth.stdout);
    assert!(stdout_depth.contains("Max recursion depth exceeded"), "Output did not contain recursion error");

    Ok(())
}

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
