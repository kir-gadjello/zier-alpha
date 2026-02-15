use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

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

fn run_zier_ask(
    root: &TempDir,
    task: &str,
    extra_env: Vec<(&str, String)>,
) -> Result<(String, String)> {
    let bin_path = env!("CARGO_BIN_EXE_zier-alpha");
    let bin_dir = root.path().join("bin");
    let home_dir = root.path().to_path_buf();
    let workspace_dir = root.path().join("workspace");

    let mut cmd = Command::new(bin_path);
    cmd.arg("ask")
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
        .current_dir(root.path());

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        anyhow::bail!("zier ask failed: {} | stderr: {}", stdout, stderr);
    }
    Ok((stdout, stderr))
}

#[test]
fn test_hive_userprompt_prefix() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path();

    // 1. Setup
    let workspace_dir = root.join("workspace");
    fs::create_dir(&workspace_dir)?;

    let ext_dir = root.join("extensions").join("hive");
    fs::create_dir_all(&ext_dir)?;
    let source_ext = PathBuf::from("extensions/hive");
    if source_ext.exists() {
        copy_dir_recursive(&source_ext, &ext_dir)?;
    } else {
        eprintln!("Hive extension source not found at {}", source_ext.display());
        return Ok(());
    }

    // Config with clone_userprompt_prefix
    let config_path = root.join("config.toml");
    let config_content = format!(
        r#"
[agent]
default_model = "mock/gpt-4o"

[extensions.hive]
enabled = true
agents_dir = "agents"
allow_clones = true
max_clone_fork_depth = 3
clone_userprompt_prefix = "[CLONE] "

[memory]
workspace = "{}"
"#,
        workspace_dir.display()
    );
    fs::write(&config_path, config_content)?;

    // Binary setup
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

    // 2. Test: parent spawns a clone with task "list files"
    let task = "list files";
    let (stdout, stderr) = run_zier_ask(
        &temp_dir,
        &format!("test_tool_json:hive_fork_subagent|{{\"agent_name\":\"\",\"task\":\"{}\"}}", task),
        vec![],
    )?;
    let combined = format!("{}\n{}", stdout, stderr);

    // Verify the spawn command includes the prefixed task
    assert!(
        combined.contains("[CLONE] list files"),
        "Expected user prompt prefix '[CLONE] ' to be applied. Combined output: {}",
        combined
    );
    // Also ensure the clone executed successfully
    assert!(stdout.contains("Mock response"), "Expected success, got: {}", stdout);

    Ok(())
}
