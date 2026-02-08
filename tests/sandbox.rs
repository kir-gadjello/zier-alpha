use localgpt::config::SandboxPolicy;
use localgpt::security::apple_sandbox::compile_profile;
use std::fs;
use std::process::Command;
use tempfile::NamedTempFile;
use std::io::Write;

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_network_denied() {
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![],
        allow_write: vec![],
    };

    // Simple python script that tries to open a socket to google.com
    let script = r#"
import socket
import sys

try:
    s = socket.create_connection(("google.com", 80), timeout=2)
    print("SUCCESS")
except Exception as e:
    print(f"FAILED: {e}")
    sys.exit(1)
"#;

    let mut script_file = NamedTempFile::new().expect("Failed to create temp script");
    script_file.write_all(script.as_bytes()).expect("Failed to write script");
    let script_path = script_file.path().to_str().unwrap();

    // Use system python3
    let executable = "/usr/bin/python3";
    let profile = compile_profile(&policy, executable, script_path);

    let mut profile_file = NamedTempFile::new().expect("Failed to create temp profile");
    profile_file.write_all(profile.as_bytes()).expect("Failed to write profile");
    let profile_path = profile_file.path().to_str().unwrap();

    // Execute with sandbox-exec
    let output = Command::new("sandbox-exec")
        .arg("-f")
        .arg(profile_path)
        .arg(executable)
        .arg(script_path)
        .output()
        .expect("Failed to run sandbox-exec");

    // We expect failure (either non-zero exit code or "FAILED" in output)
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("FAILED"), "Network should be denied, but got: {}", stdout);
    } else {
        // Non-zero exit code is also acceptable (e.g. killed by sandbox)
    }
}

#[test]
#[cfg(target_os = "macos")]
fn test_sandbox_write_denied() {
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![],
        allow_write: vec![], // No write allowed
    };

    let script = r#"
try:
    with open("/tmp/sandbox_test_write.txt", "w") as f:
        f.write("test")
    print("SUCCESS")
except Exception as e:
    print(f"FAILED: {e}")
"#;

    let mut script_file = NamedTempFile::new().expect("Failed to create temp script");
    script_file.write_all(script.as_bytes()).expect("Failed to write script");
    let script_path = script_file.path().to_str().unwrap();

    let executable = "/usr/bin/python3";
    let profile = compile_profile(&policy, executable, script_path);

    let mut profile_file = NamedTempFile::new().expect("Failed to create temp profile");
    profile_file.write_all(profile.as_bytes()).expect("Failed to write profile");
    let profile_path = profile_file.path().to_str().unwrap();

    let output = Command::new("sandbox-exec")
        .arg("-f")
        .arg(profile_path)
        .arg(executable)
        .arg(script_path)
        .output()
        .expect("Failed to run sandbox-exec");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if output.status.success() {
         assert!(stdout.contains("FAILED"), "Write should be denied, but got: {}", stdout);
    }
}
