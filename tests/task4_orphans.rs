use anyhow::Result;
use std::process::Command;
use std::time::Duration;

#[tokio::test]
async fn test_orphan_cleanup_kill_on_drop() -> Result<()> {
    if cfg!(windows) {
        return Ok(());
    }

    let mut cmd = tokio::process::Command::new("sleep");
    cmd.arg("100");
    cmd.kill_on_drop(true);

    let child = cmd.spawn()?;
    let pid = child.id().expect("Child must have PID");

    // Drop the child handle
    drop(child);

    // Wait for cleanup
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check if process exists using `kill -0 <pid>`
    let status = Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .output()?;

    // kill -0 returns 0 (success) if process exists.
    // It returns 1 (failure) if process does not exist (or permission denied).
    // We expect FAILURE (process gone).

    if status.status.success() {
        // Process exists! Kill it manually.
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
        panic!("Child process {} was not killed on drop", pid);
    }

    Ok(())
}
