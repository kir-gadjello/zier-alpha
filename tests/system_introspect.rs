use tempfile::TempDir;
use zier_alpha::agent::disk_monitor::DiskMonitor;
use zier_alpha::agent::mcp_manager::McpManager;
use zier_alpha::agent::tools::system::SystemIntrospectTool;
use zier_alpha::agent::Tool;
use zier_alpha::config::{Config, DiskConfig};
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_system_introspect_tool() {
    let temp = TempDir::new().unwrap();
    let config = Config::default();

    // Mock dependencies
    let mcp_manager = McpManager::new(600);
    let disk_monitor = DiskMonitor::new(DiskConfig::default());

    // ScriptService (requires some setup, maybe mock or minimal)
    // ScriptService::new spawns a thread.
    let service = ScriptService::new(
        zier_alpha::config::SandboxPolicy::default(),
        temp.path().to_path_buf(),
        temp.path().to_path_buf(),
        zier_alpha::config::WorkdirStrategy::Overlay,
        None,
        None,
        None,
        "test".to_string(),
    )
    .unwrap();

    let tool = SystemIntrospectTool::new(config, mcp_manager, service, disk_monitor);

    // Test status
    let result = tool.execute(r#"{"command": "status"}"#).await.unwrap();
    assert!(result.contains("version"));
    assert!(result.contains("degraded_mode"));

    // Test mcp
    let result = tool.execute(r#"{"command": "mcp"}"#).await.unwrap();
    assert!(result.contains("[")); // Empty array

    // Test cleanup_disk: should report completion (message contains "completed" or "Deleted")
    let result = tool
        .execute(r#"{"command": "cleanup_disk"}"#)
        .await
        .unwrap();
    assert!(
        result.contains("completed") || result.contains("Deleted"),
        "cleanup_disk result: {}",
        result
    );
}
