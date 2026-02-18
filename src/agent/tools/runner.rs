use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{debug, warn};
use crate::config::SandboxPolicy;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

#[cfg(target_os = "macos")]
pub async fn run_sandboxed_command(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    env: Option<std::collections::HashMap<String, String>>,
    policy: &SandboxPolicy,
) -> Result<std::process::Output> {
    if !policy.enable_os_sandbox {
        return run_direct(command, args, cwd, env).await;
    }

    // Determine script path (heuristic: first arg ending in .py, .sh, .js, etc. or just first arg)
    let script_path = args.first().map(|s| s.as_str()).unwrap_or("unknown");

    // Generate profile
    let profile_content = crate::security::compile_profile(policy, command, script_path);

    // Write profile to temp file
    let mut temp_file = tempfile::Builder::new()
        .prefix("zier_sandbox")
        .suffix(".sb")
        .tempfile()?;

    use std::io::Write;
    temp_file.write_all(profile_content.as_bytes())?;
    let profile_path = temp_file.path().to_path_buf();

    // Keep temp file alive until command finishes?
    // tempfile deletes on drop. So we must keep `temp_file` in scope.

    debug!("Running sandboxed command with profile: {}", profile_path.display());

    let mut cmd = Command::new("sandbox-exec");
    cmd.arg("-f").arg(&profile_path);
    cmd.arg(command);
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.kill_on_drop(true);

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let output = cmd.output().await.context("Failed to run sandboxed command on macOS")?;

    // Explicitly keep temp_file alive until here
    drop(temp_file);

    Ok(output)
}

#[cfg(target_os = "linux")]
pub async fn run_sandboxed_command(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    env: Option<std::collections::HashMap<String, String>>,
    policy: &SandboxPolicy,
) -> Result<std::process::Output> {
    if !policy.enable_os_sandbox {
        return run_direct(command, args, cwd, env).await;
    }

    // Check for bubblewrap
    let has_bwrap = Command::new("bwrap")
        .arg("--version")
        .output()
        .await
        .is_ok();

    if has_bwrap {
        debug!("Using bubblewrap for sandboxing");
        let mut cmd = Command::new("bwrap");
        cmd.kill_on_drop(true);

        // Basic flags
        // --die-with-parent ensures child death if parent dies (on Linux)
        cmd.args(&["--unshare-all", "--die-with-parent"]);

        // Ensure PDEATHSIG is set if we are not using bwrap?
        // But bwrap handles it via --die-with-parent.
        // For direct execution or unshare, we need unsafe pre_exec.

        // Filesystem Bindings
        // Read-only system paths
        cmd.args(&["--ro-bind", "/usr", "/usr"]);
        cmd.args(&["--ro-bind", "/bin", "/bin"]);
        cmd.args(&["--ro-bind", "/lib", "/lib"]);
        if std::path::Path::new("/lib64").exists() {
            cmd.args(&["--ro-bind", "/lib64", "/lib64"]);
        }
        cmd.args(&["--ro-bind", "/etc/alternatives", "/etc/alternatives"]); // Common for java/etc
        cmd.args(&["--ro-bind", "/etc/ssl", "/etc/ssl"]); // For HTTPS
        cmd.args(&["--ro-bind", "/etc/resolv.conf", "/etc/resolv.conf"]); // For DNS

        // Tmpfs
        cmd.args(&["--tmpfs", "/tmp"]);
        cmd.args(&["--dev", "/dev"]);
        cmd.args(&["--proc", "/proc"]);

        // Allow read paths
        for path in &policy.allow_read {
            let expanded = shellexpand::tilde(path).to_string();
            if std::path::Path::new(&expanded).exists() {
                 cmd.args(&["--ro-bind", &expanded, &expanded]);
            }
        }

        // Allow write paths
        for path in &policy.allow_write {
            let expanded = shellexpand::tilde(path).to_string();
            if std::path::Path::new(&expanded).exists() {
                 cmd.args(&["--bind", &expanded, &expanded]);
            }
        }

        // Always bind CWD if not covered?
        // Usually workspace is in allow_write, but verify.
        // Assuming CWD is safe/needed.
        // If CWD is not in allow list, we might break things.
        // Let's bind CWD as read-write for now if it exists, or just rely on policy?
        // The policy should cover it.
        // But to be safe for "project" dir:
        if cwd.exists() {
             cmd.args(&["--bind", cwd.to_str().unwrap(), cwd.to_str().unwrap()]);
        }

        // Network
        if policy.allow_network {
            cmd.args(&["--share-net"]);
        }

        // Command
        cmd.arg(command);
        cmd.args(args);

        // Env
        // bwrap clears env by default. We need to pass them.
        // --setenv VAR VALUE
        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                cmd.arg("--setenv");
                cmd.arg(k);
                cmd.arg(v);
            }
        }

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.output().await.context("Failed to run bwrap command")
    } else {
        warn!("Bubblewrap not found, falling back to unshare (weaker sandbox)");
        // Fallback to unshare
        let mut cmd = Command::new("unshare");
        cmd.kill_on_drop(true);

        // Ensure child dies if parent dies
        unsafe {
            cmd.pre_exec(|| {
                // PR_SET_PDEATHSIG = 1
                let r = libc::prctl(1, libc::SIGKILL);
                if r != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        // User namespace (-r) maps root to current user, giving capability to mount
        // Mount namespace (-m)
        // Net namespace (-n) if no network
        cmd.args(&["-r", "-m"]);
        if !policy.allow_network {
            cmd.arg("-n");
        }

        // We can't easily do bind mounts with just unshare command wrapper without a script.
        // We will just run the command in the namespace.
        cmd.arg(command);
        cmd.args(args);
        cmd.current_dir(cwd);

        if let Some(env_vars) = env {
            cmd.envs(env_vars);
        }

        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        cmd.output().await.context("Failed to run unshare command")
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub async fn run_sandboxed_command(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    env: Option<std::collections::HashMap<String, String>>,
    _policy: &SandboxPolicy,
) -> Result<std::process::Output> {
    warn!("Sandboxing not supported on this platform, running command directly");
    run_direct(command, args, cwd, env).await
}

async fn run_direct(
    command: &str,
    args: &[String],
    cwd: &PathBuf,
    env: Option<std::collections::HashMap<String, String>>,
) -> Result<std::process::Output> {
    let mut cmd = Command::new(command);
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.kill_on_drop(true);

    #[cfg(target_os = "linux")]
    unsafe {
        cmd.pre_exec(|| {
            // PR_SET_PDEATHSIG = 1
            let r = libc::prctl(1, libc::SIGKILL);
            if r != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    if let Some(env_vars) = env {
        cmd.envs(env_vars);
    }

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.output().await.context("Failed to run command")
}
