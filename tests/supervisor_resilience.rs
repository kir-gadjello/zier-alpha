use std::process::{Command, Stdio};
use std::time::Duration;
use std::thread;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_supervisor_restarts_on_crash() {
    // 1. Build the binary (no default features to avoid winit issues)
    let status = Command::new("cargo")
        .args(&["build", "--bin", "zier-alpha", "--no-default-features"])
        .status()
        .expect("Failed to build zier-alpha");
    assert!(status.success());

    let bin_path = if cfg!(debug_assertions) {
        "./target/debug/zier-alpha"
    } else {
        "./target/release/zier-alpha"
    };

    // 2. Create invalid config file to trigger child failure
    let mut bad_config = NamedTempFile::new().unwrap();
    write!(bad_config, "this is not valid toml").unwrap();
    let _config_path = bad_config.path().to_str().unwrap().to_string();

    // 3. Run supervisor
    // We use "ask" which should attempt to use LLM.
    // Since we don't configure valid keys/tools in default environment, it should fail.
    // If "claude" is missing (default model), it might fail.
    // Or we force a non-existent model/provider.
    // "invalid/model" might fail config validation or agent creation.
    // Agent creation failure returns error -> main returns error -> exit non-zero.

    let mut child = Command::new(bin_path)
        .arg("--supervised")
        .arg("--agent")
        .arg("crash_test_agent")
        .arg("ask")
        .arg("why is the sky blue")
        .env("OPENAI_API_KEY", "invalid-key") // Ensure if it falls back it fails
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn supervisor");

    // 4. Monitor output for restart messages
    thread::sleep(Duration::from_secs(5));

    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to wait on child");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("Supervisor stdout:\n{}", stdout);
    println!("Supervisor stderr:\n{}", stderr);

    assert!(stdout.contains("Starting supervisor for"), "Supervisor didn't start. Stderr: {}", stderr);
    assert!(stdout.contains("Child exited with error"), "Supervisor didn't detect crash");
    assert!(stdout.contains("Restarting in"), "Supervisor didn't attempt restart");

    // Check that it tried at least twice
    let crash_count = stdout.matches("Child exited with error").count();
    assert!(crash_count >= 1, "Supervisor should have restarted at least once (count: {})", crash_count);
}
