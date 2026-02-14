use std::process::{Command, Stdio};
use std::time::Duration;
use std::thread;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_supervisor_restarts_on_crash() {
    // Build the binary (no default features to avoid winit issues)
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

    // Create a temporary HOME directory with an invalid config file
    let temp_home = TempDir::new().unwrap();
    let home_dir = temp_home.path();
    let config_dir = home_dir.join(".zier-alpha");
    std::fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("config.toml");
    std::fs::write(&config_path, "this is not valid toml").unwrap();

    // Run supervisor with HOME pointing to the invalid config
    let mut child = Command::new(bin_path)
        .arg("--supervised")
        .arg("--agent")
        .arg("crash_test_agent")
        .arg("ask")
        .arg("why is the sky blue")
        .env("HOME", home_dir) // Use temporary HOME with invalid config
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn supervisor");

    // Wait a few seconds for child to start and crash
    thread::sleep(Duration::from_secs(5));

    // Kill the supervisor (it will keep restarting, but we stop after 5s)
    let _ = child.kill();
    let output = child.wait_with_output().expect("Failed to wait on child");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("Supervisor stdout:\n{}", stdout);
    println!("Supervisor stderr:\n{}", stderr);

    // Supervisor should have started and detected at least one crash/restart cycle
    assert!(stdout.contains("Starting supervisor for"), "Supervisor didn't start. Stderr: {}", stderr);
    assert!(stdout.contains("Child exited with error"), "Supervisor didn't detect any crash. stdout: {}", stdout);
    assert!(stdout.contains("Restarting in"), "Supervisor didn't attempt restart");
    let crash_count = stdout.matches("Child exited with error").count();
    assert!(crash_count >= 1, "Supervisor should have restarted at least once (count: {})", crash_count);
}
