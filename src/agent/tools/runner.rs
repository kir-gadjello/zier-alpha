use anyhow::{Result, Context};
use std::process::Stdio;
use tokio::process::Command;
use std::path::PathBuf;

#[cfg(target_os = "macos")]
pub async fn run_sandboxed_command(command: &str, args: &[String], cwd: &PathBuf, env: Option<std::collections::HashMap<String, String>>) -> Result<std::process::Output> {
    let profile = r#"
(version 1)
(allow default)
(deny network*)
(allow file-read* (subpath "/"))
(allow file-write* (subpath "/tmp"))
"#;
    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-p").arg(profile);
    cmd.arg(command);
    cmd.args(args);
    cmd.current_dir(cwd);

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.output().await.context("Failed to run sandboxed command on macOS")
}

#[cfg(target_os = "linux")]
pub async fn run_sandboxed_command(command: &str, args: &[String], cwd: &PathBuf, env: Option<std::collections::HashMap<String, String>>) -> Result<std::process::Output> {
    // Linux unshare
    let mut cmd = Command::new("unshare");
    cmd.args(&["-n", "-i", "-u", "-p", "-f", "--mount-proc"]);
    cmd.arg(command);
    cmd.args(args);
    cmd.current_dir(cwd);

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.output().await.context("Failed to run sandboxed command on Linux")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn run_sandboxed_command(command: &str, args: &[String], cwd: &PathBuf, env: Option<std::collections::HashMap<String, String>>) -> Result<std::process::Output> {
    warn!("Sandboxing not supported on this platform, running command directly");
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.current_dir(cwd);

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.output().await.context("Failed to run command")
}
