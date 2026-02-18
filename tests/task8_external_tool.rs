use anyhow::Result;
use zier_alpha::agent::tools::external::ExternalTool;
use zier_alpha::agent::tools::Tool;
use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};

#[tokio::test]
async fn test_external_tool_path_resolution() -> Result<()> {
    if cfg!(windows) {
        return Ok(());
    }

    let temp_dir = tempfile::tempdir()?;
    let workspace = temp_dir.path().join("workspace");
    let project = temp_dir.path().join("project");

    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&project)?;

    // Create an external tool: "echo"
    // args: ["prefix"]
    // path_args: ["file"]

    let tool = ExternalTool::new(
        "my_echo".to_string(),
        "Echoes path".to_string(),
        "echo".to_string(),
        vec!["prefix".to_string()],
        Some(project.clone()),
        false, // no sandbox for simple test
        Some(SandboxPolicy::default()),
        vec!["file".to_string()],
        Some(workspace.clone()),
        Some(WorkdirStrategy::Overlay),
    );

    // 1. Call with relative path in "file"
    let args_json = serde_json::json!({
        "file": "foo.txt"
    }).to_string();

    let output = tool.execute(&args_json).await?;

    // Expected behavior:
    // echo prefix /path/to/project/foo.txt (Overlay strategy -> project for non-cognitive)
    // Actually resolved path depends on resolve_path logic.
    // "foo.txt" is not cognitive, so it goes to project dir.

    let expected_path = project.join("foo.txt").canonicalize().unwrap_or(project.join("foo.txt"));

    assert!(output.contains("prefix"));
    assert!(output.contains(expected_path.to_str().unwrap()));

    Ok(())
}
