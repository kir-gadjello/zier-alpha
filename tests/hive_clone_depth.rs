use anyhow::Result;
use chrono::Utc;
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

/// Run `zier ask` with optional extra environment variables.
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
fn test_hive_clone_depth_limit() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let root = temp_dir.path();

    // 1. Environment setup
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

    // Config with clone depth limit = 2
    let config_path = root.join("config.toml");
    let config_content = format!(
        r#"
[agent]
default_model = "mock/gpt-4o"

[extensions.hive]
enabled = true
agents_dir = "agents"
allow_clones = true
max_clone_fork_depth = 2

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

    // Create a mock session file for hydration
    let session_id = "test-session";
    let sessions_dir = dot_zier.join("agents").join("main").join("sessions");
    fs::create_dir_all(&sessions_dir)?;
    let timestamp = Utc::now().to_rfc3339();
    let session_content = format!(
        r#"{{"type":"session","version":1,"id":"{0}","timestamp":"{1}","cwd":"{2}"}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"The secret code is 42"}}]}}}}
"#,
        session_id,
        timestamp,
        root.display()
    );
    fs::write(
        sessions_dir.join(format!("{}.jsonl", session_id)),
        session_content,
    )?;

    // 2. Test: Depth 1 (parent not clone) -> spawn clone should succeed
    let task1 = r#"test_tool_json:hive_fork_subagent|{"agent_name":"","task":"hello"}"#;
    let (stdout1, stderr1) = run_zier_ask(
        &temp_dir,
        task1,
        vec![("ZIER_SESSION_ID", session_id.to_string())],
    )?;
    let combined1 = format!("{}\n{}", stdout1, stderr1);
    assert!(
        stdout1.contains("Mock response"),
        "Expected success for depth 1 clone, got: {}",
        stdout1
    );
    assert!(
        combined1.contains("[Hive] Spawning:"),
        "Expected spawn log, got: {}",
        combined1
    );

    // 3. Test: Parent with ZIER_HIVE_CLONE_DEPTH=2 -> attempting to spawn a clone (would be depth 3) should fail
    let (stdout2, stderr2) = run_zier_ask(
        &temp_dir,
        task1,
        vec![
            ("ZIER_SESSION_ID", session_id.to_string()),
            ("ZIER_HIVE_CLONE_DEPTH", "2".to_string()),
        ],
    )?;
    let _combined2 = format!("{}\n{}", stdout2, stderr2);
    assert!(
        stdout2.contains("Max clone fork depth exceeded")
            || stderr2.contains("Max clone fork depth exceeded"),
        "Expected depth limit error in output. stdout: {}, stderr: {}",
        stdout2,
        stderr2
    );

    Ok(())
}
