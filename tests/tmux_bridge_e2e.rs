use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::scripting::ScriptService;
use zier_alpha::ingress::IngressBus;
use zier_alpha::scheduler::Scheduler;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::path::PathBuf;

#[tokio::test]
async fn test_tmux_bridge_lifecycle() {
    // 1. Setup
    let temp_dir = tempfile::tempdir().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    // Initialize services
    let bus = Arc::new(IngressBus::new(100));
    let mut scheduler = Scheduler::new(bus.clone()).await.unwrap();
    scheduler.start().await.unwrap();
    let scheduler = Arc::new(Mutex::new(scheduler));

    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec!["/".to_string()], // Allow all for test convenience
        allow_write: vec![workspace.to_string_lossy().to_string()],
    };

    let service = ScriptService::new(
        policy,
        workspace.clone(),
        workspace.clone(),
        WorkdirStrategy::Overlay,
        Some(bus.clone()),
        Some(scheduler.clone())
    ).unwrap();

    // Load plugin from the REPO location
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let plugin_path = PathBuf::from(manifest_dir).join("extensions/tmux_bridge/main.js");

    if !plugin_path.exists() {
        panic!("Plugin not found at {}", plugin_path.display());
    }

    service.load_script(plugin_path.to_str().unwrap()).await.expect("Failed to load plugin");

    // 2. Test Spawn
    println!("Spawning test_server...");
    let spawn_args = serde_json::json!({
        "name": "test_server",
        "command": "sleep 10"
    }).to_string();

    let result = service.execute_tool("tmux_spawn", &spawn_args).await.unwrap();
    println!("Spawn result: {}", result);

    assert!(result.contains("process_started"));
    assert!(result.contains("test_server"));
    assert!(result.contains("running"));

    // 3. Test Inspect
    println!("Inspecting test_server...");
    let inspect_args = serde_json::json!({
        "id": "test_server",
        "mode": "full_status"
    }).to_string();

    let inspect = service.execute_tool("tmux_inspect", &inspect_args).await.unwrap();
    println!("Inspect result: {}", inspect);

    assert!(inspect.contains("running"));

    // 4. Test Control (Write) - useless for sleep but tests mechanism
    println!("Writing to test_server...");
    let write_args = serde_json::json!({
        "id": "test_server",
        "action": "write",
        "payload": "echo hello"
    }).to_string();

    let write = service.execute_tool("tmux_control", &write_args).await.unwrap();
    println!("Write result: {}", write);
    assert!(write.contains("success"));

    // 5. Test History
    println!("Querying history...");
    let history_args = serde_json::json!({
        "id": "test_server"
    }).to_string();

    let history = service.execute_tool("tmux_history", &history_args).await.unwrap();
    println!("History result: {}", history);
    assert!(history.contains("spawn"));

    // 6. Test Kill
    println!("Killing test_server...");
    let kill_args = serde_json::json!({
        "id": "test_server",
        "action": "kill"
    }).to_string();

    let kill = service.execute_tool("tmux_control", &kill_args).await.unwrap();
    println!("Kill result: {}", kill);
    assert!(kill.contains("success"));

    // 7. Verify Dead
    let inspect_dead = service.execute_tool("tmux_inspect", &inspect_args).await.unwrap();
    assert!(inspect_dead.contains("error")); // State removed
    // Or if we check tmux directly?

    // 8. Test Status Hook
    let status_lines = service.get_status_lines().await.unwrap();
    println!("Status lines: {:?}", status_lines);
    // Should be empty or header only as session is killed

    // 9. Test Scheduler Registration (Monitor)
    println!("Adding monitor...");
    // We need a session for monitor
    service.execute_tool("tmux_spawn", &spawn_args).await.unwrap();

    let monitor_args = serde_json::json!({
        "id": "test_server",
        "pattern": "ERROR"
    }).to_string();

    let monitor = service.execute_tool("tmux_monitor", &monitor_args).await.unwrap();
    println!("Monitor result: {}", monitor);
    assert!(monitor.contains("monitor_added"));

    // Verify monitor daemon registered
    // We can't easily access scheduler internal state, but we assume it didn't crash.

    // Clean up last session
    service.execute_tool("tmux_control", &kill_args).await.unwrap();
}
