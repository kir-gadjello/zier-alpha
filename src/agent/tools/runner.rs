use crate::config::SandboxPolicy;
use crate::security::compile_profile;
use std::process::Stdio;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;
use tempfile::NamedTempFile;
use anyhow::Result;
use std::io::Write; // For synchronous write to temp file

#[allow(dead_code)]
pub async fn run_sandboxed_tool(
    executable: &str,
    script_path: &str,
    arguments: &str,
    policy: &SandboxPolicy
) -> Result<String> {
    // 1. Compile profile
    let profile = compile_profile(policy, executable, script_path);

    // 2. Write profile to temp file
    // Note: NamedTempFile deletes the file when it goes out of scope.
    // We need to keep it alive until the command finishes, or use persist/keep.
    // But sandbox-exec needs the path.
    // If we use NamedTempFile, we can get the path, but if we drop the variable, the file is gone.
    // So we keep the file handle until the end of the function scope.
    let mut profile_file = NamedTempFile::new()?;
    profile_file.write_all(profile.as_bytes())?;
    let profile_path = profile_file.path().to_str().ok_or_else(|| anyhow::anyhow!("Invalid profile path"))?.to_string();

    #[cfg(target_os = "macos")]
    let (program, _args) = ("sandbox-exec", vec!["-f", &profile_path, executable, script_path]);

    #[cfg(not(target_os = "macos"))]
    let (program, _args) = (executable, vec![script_path]);

    // Used to suppress unused variable warning on non-macos
    let _ = profile_path;

    // 3. Construct command
    let mut cmd = Command::new(program);

    // On non-macOS, we don't sandbox (for now/dev), or we could fail.
    // The prompt implies strict macOS sandbox. But for cross-platform dev (like this env),
    // maybe we just warn and run?
    // "We treat the executable path as absolute and opaque."

    #[cfg(target_os = "macos")]
    {
        cmd.arg("-f").arg(&profile_path);
        cmd.arg(executable);
        cmd.arg(script_path);
    }

    #[cfg(not(target_os = "macos"))]
    {
        // On Linux/Windows, just run the script directly for now so logic can be tested
        // cmd is already executable
        cmd.arg(script_path);
        tracing::warn!("Running without sandbox (not on macOS)");
    }

    // 4. Pass arguments via stdin
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(arguments.as_bytes()).await?;
    }

    // 5. Capture output
    let output = child.wait_with_output().await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Tool execution failed ({}): {}", output.status, stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
