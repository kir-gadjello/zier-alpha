use anyhow::Result;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use zier_alpha::agent::tools::{ReadFileTool, Tool, WriteFileTool};
use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::agent::DiskMonitor;
use zier_alpha::config::DiskConfig;

#[tokio::test]
async fn test_builtin_tool_path_permission() -> Result<()> {
    // Setup temp dirs
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();
    let workspace = root.join("workspace");
    let project = root.join("project");
    let outside = root.join("outside");

    fs::create_dir_all(&workspace)?;
    fs::create_dir_all(&project)?;
    fs::create_dir_all(&outside)?;

    // Create files
    fs::write(workspace.join("safe.txt"), "safe workspace")?;
    fs::write(project.join("safe.txt"), "safe project")?;
    fs::write(outside.join("secret.txt"), "secret")?;

    // Create default policy (empty allow lists)
    let policy = SandboxPolicy::default();

    // Create tools
    let read_tool = ReadFileTool::new(
        workspace.clone(),
        project.clone(),
        WorkdirStrategy::Overlay,
        policy.clone(),
    );

    let disk_monitor = DiskMonitor::new(DiskConfig::default());
    let write_tool = WriteFileTool::new(
        workspace.clone(),
        project.clone(),
        WorkdirStrategy::Overlay,
        disk_monitor,
        policy.clone(),
    );

    // Test 1: Read workspace file (allowed)
    // resolve_path treats absolute paths as absolute.
    let workspace_file = workspace.join("safe.txt");
    let args = serde_json::json!({
        "path": workspace_file.to_str().unwrap()
    }).to_string();

    let res = read_tool.execute(&args).await;
    assert!(res.is_ok(), "Should read workspace file");
    assert!(res.unwrap().contains("safe workspace"));

    // Test 2: Read outside file (denied)
    let outside_file = outside.join("secret.txt");
    let args_outside = serde_json::json!({
        "path": outside_file.to_str().unwrap()
    }).to_string();

    let res_outside = read_tool.execute(&args_outside).await;
    assert!(res_outside.is_err(), "Should deny access to outside file");
    let err_msg = res_outside.unwrap_err().to_string();
    assert!(err_msg.contains("Path access denied"), "Error should be about access denied, got: {}", err_msg);

    // Test 3: Write to outside file (denied)
    let outside_write = outside.join("new_secret.txt");
    let args_write = serde_json::json!({
        "path": outside_write.to_str().unwrap(),
        "content": "hacked"
    }).to_string();

    let res_write = write_tool.execute(&args_write).await;
    assert!(res_write.is_err(), "Should deny write to outside file");
    assert!(res_write.unwrap_err().to_string().contains("Path access denied"));

    // Test 4: Allow list
    let mut allowed_policy = SandboxPolicy::default();
    allowed_policy.allow_read.push(outside_file.to_str().unwrap().to_string());
    allowed_policy.allow_write.push(outside_write.to_str().unwrap().to_string());

    let allowed_read_tool = ReadFileTool::new(
        workspace.clone(),
        project.clone(),
        WorkdirStrategy::Overlay,
        allowed_policy.clone(),
    );

    let allowed_write_tool = WriteFileTool::new(
        workspace.clone(),
        project.clone(),
        WorkdirStrategy::Overlay,
        DiskMonitor::new(DiskConfig::default()),
        allowed_policy.clone(),
    );

    let res_allowed_read = allowed_read_tool.execute(&args_outside).await;
    assert!(res_allowed_read.is_ok(), "Should allow read access if in policy");
    assert!(res_allowed_read.unwrap().contains("secret"));

    let res_allowed_write = allowed_write_tool.execute(&args_write).await;
    assert!(res_allowed_write.is_ok(), "Should allow write access if in policy");

    Ok(())
}
