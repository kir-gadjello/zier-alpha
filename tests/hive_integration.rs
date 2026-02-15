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
        eprintln!(
            "Hive extension source not found at {}",
            source_ext.display()
        );
        // In some CI envs, cwd might be different. Try finding it.
        // But for this environment, it should be present.
        return Ok(());
    }

    // Create config
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

    // Create a test agent in the "agents" dir (scanned by registry)
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

    // 2. Run Test: hive_fresh_basic
    // Trigger delegation via mock tool call
    let bin_path = env!("CARGO_BIN_EXE_zier-alpha");

    // Create 'zier' symlink in the same dir as the binary (target/debug/...)
    // Note: We might not have write permission there in some environments, or parallel tests might conflict.
    // Better to create a symlink in our temp dir and add temp dir to PATH.
    let bin_dir = root.join("bin");
    fs::create_dir(&bin_dir)?;

    #[cfg(unix)]
    std::os::unix::fs::symlink(bin_path, bin_dir.join("zier"))?;
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
        .arg("test_tool_json:hive_fork_subagent|{\"agent_name\": \"echo\", \"task\": \"hello\"}")
        .env("HOME", &home_dir) // Force Config::load to use our config
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        // Add our temp bin dir to PATH so 'zier' is found
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .current_dir(root)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let _stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        panic!("zier ask failed with status: {:?}", output.status);
    }

    assert!(
        stdout.contains("Mock response"),
        "Output did not contain expected mock response"
    );

    // 3. Test: hive_depth_limit
    let output_depth = Command::new(bin_path)
        .arg("ask")
        .arg("test_tool_json:hive_fork_subagent|{\"agent_name\": \"echo\", \"task\": \"hello\"}")
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
        .env("ZIER_HIVE_DEPTH", "3")
        .current_dir(root)
        .output()?;

    let stdout_depth = String::from_utf8_lossy(&output_depth.stdout);
    assert!(
        stdout_depth.contains("Max Hive recursion depth exceeded"),
        "Output did not contain recursion error"
    );

    // 4. Test: hive_fork_context
    // Create a mock session file in the expected path: ~/.zier-alpha/agents/main/sessions/{id}.jsonl
    let agent_id = "main";
    let session_id = "test-session";
    let sessions_dir = dot_zier.join("agents").join(agent_id).join("sessions");
    fs::create_dir_all(&sessions_dir)?;

    let session_path = sessions_dir.join(format!("{}.jsonl", session_id));
    // Write a session file with a user message "The secret code is 42"
    let timestamp = chrono::Utc::now().to_rfc3339();
    let session_content = format!(
        r#"{{"type":"session","version":1,"id":"{0}","timestamp":"{1}","cwd":"{2}"}}
{{"type":"message","message":{{"role":"user","content":[{{"type":"text","text":"The secret code is 42"}}]}}}}
"#,
        session_id,
        timestamp,
        root.display()
    );
    fs::write(&session_path, session_content)?;

    // Run ask with ZIER_SESSION_ID set, triggering fork mode
    let output_fork = Command::new(bin_path)
        .arg("ask")
        // We instruct the subagent to recall the secret code
        .arg("test_tool_json:hive_fork_subagent|{\"agent_name\": \"echo\", \"task\": \"What is the secret code?\", \"context_mode\": \"fork\"}")
        .env("HOME", &home_dir)
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        .env("ZIER_SESSION_ID", session_id)
        .env("PATH", format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default()))
        .current_dir(root)
        .output()?;

    let stdout_fork = String::from_utf8_lossy(&output_fork.stdout);
    println!("STDOUT_FORK: {}", stdout_fork);

    // In our MockProvider, we just return "Mock response".
    // To verify context hydration actually happened, we need the Child process to have the context.
    // The child process receives "--hydrate-from".
    // The Agent::hydrate_from_file loads it.
    // The Agent::chat sends messages to LLM.
    // The MockProvider sees the messages.
    // We can't easily assert the internal state of the child process from here unless the child echoes it back.
    // But our "echo" agent uses "mock/gpt-4o".
    // The "echo" agent description says "You are EchoBot." but that's system prompt.
    // The MockProvider returns static "Mock response" unless specific triggers.
    // WE need the MockProvider to return the context if asked?
    // MockProvider doesn't implement context search.
    // However, if hydration works, the "messages" passed to `chat` will include "The secret code is 42".
    // We can update MockProvider to look for "What is the secret code?" and if prior message has "42", return it?
    //
    // Let's rely on the fact that `orchestrator.js` logs "Spawning: ... --hydrate-from ...".
    // But we capture parent output. We don't see child logs easily unless we redirect child stdout/stderr.
    // Wait, child stdout is JSON output (ipc). Child stderr goes to parent stderr.
    // We can check stderr for hydration log?
    // "Hydrated session from ..." is logged at INFO level.
    // We need to enable logging? RUST_LOG=info.

    // Let's enable RUST_LOG for the fork test
    // Rerun fork test with logging
    let output_fork_log = Command::new(bin_path)
        .arg("ask")
        .arg("test_tool_json:hive_fork_subagent|{\"agent_name\": \"echo\", \"task\": \"Context check\", \"context_mode\": \"fork\"}")
        .env("HOME", &home_dir)
        .env("ZIER_ALPHA_WORKSPACE", &workspace_dir)
        .env("ZIER_SESSION_ID", session_id)
        .env("PATH", format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default()))
        .env("RUST_LOG", "info")
        .current_dir(root)
        .output()?;

    let stderr_fork = String::from_utf8_lossy(&output_fork_log.stderr);
    println!("STDERR_FORK: {}", stderr_fork);
    // We expect the CHILD process to log "Hydrated session from".
    // But child stderr is captured by `orchestrator.js` only on error?
    // No, `exec` inherits?
    // In `deno.rs`, `op_zier_exec` uses `command.output().await?`. This captures stdout/stderr.
    // The orchestrator throws if code != 0.
    // If success, it returns content. It doesn't print child stderr to parent stderr.
    // So we can't see child logs.

    // Modification: We can verify the IPC file logic or just assume if it didn't crash and we passed the flag, it worked?
    // Weak verification.
    // Ideally, the "echo" agent would actually echo the history.
    // But we are using MockProvider.
    // If we change MockProvider to be smarter?
    // Or we verify that the temporary hydration file was created?
    // It is deleted after use.

    // Let's assume if it runs without error, the hydration path parsing worked.
    // The orchestrator throws if it fails to write the hydration file.

    assert!(output_fork.status.success());
    assert!(
        stdout_fork.contains("The secret code is 42"),
        "Expected hydrated response with secret, got: {}",
        stdout_fork
    );

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
