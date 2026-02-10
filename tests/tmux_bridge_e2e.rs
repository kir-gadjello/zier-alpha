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

    // 10. Test Security Blocking (Command Heuristic)
    println!("Testing security block (rm -rf)...");
    let rm_args = serde_json::json!({
        "name": "dangerous",
        "command": "rm -rf /"
    }).to_string();

    let rm_result = service.execute_tool("tmux_spawn", &rm_args).await;
    // The tool returns <error> in stdout, not Err in result
    let rm_xml = rm_result.unwrap();
    println!("Security check result: {}", rm_xml);
    assert!(rm_xml.contains("error"));
    // Allow either "safety policy" (if mapped correctly) or "invalid_argument" (if mapped to EINVAL)
    // But we prefer "safety policy".
    if !rm_xml.contains("safety policy") {
        println!("WARNING: Error message did not contain 'safety policy', got: {}", rm_xml);
        // If it's invalid_argument, accept it for now but note it's suboptimal.
        // assert!(rm_xml.contains("invalid_argument"));
        // Actually, let's enforce "safety policy" because I changed ErrorKind to Other.
        assert!(rm_xml.contains("safety policy"));
    }

    // 11. Test Expect Tool (Interactive Automation)
    println!("Testing expect tool...");

    // Create a script to avoid shell chaining violation (since ; and && are blocked)
    let script_path = workspace.join("interactive.py");
    std::fs::write(&script_path, "import time; time.sleep(1); print('Enter password:', flush=True); time.sleep(5)").unwrap();

    let expect_server_args = serde_json::json!({
        "name": "interactive_server",
        "command": format!("python3 {}", script_path.to_string_lossy())
    }).to_string();

    let spawn_res = service.execute_tool("tmux_spawn", &expect_server_args).await.unwrap();
    println!("Spawn interactive: {}", spawn_res);
    assert!(!spawn_res.contains("error"));

    let expect_args = serde_json::json!({
        "id": "interactive_server",
        "pattern": "Enter password:",
        "send": "my_secret",
        "timeout": 3000
    }).to_string();

    let expect_res = service.execute_tool("tmux_expect", &expect_args).await.unwrap();
    println!("Expect result: {}", expect_res);

    assert!(expect_res.contains("<success>true</success>"));
    assert!(expect_res.contains("sent response"));

    // Verify input was received by checking history?
    // tmux send-keys puts input into the pane, so it should be visible in history if echo is on.
    // Or at least verify the expect tool claimed success.

    // 12. Test Diff Tool
    println!("Testing diff tool...");
    let diff_args = serde_json::json!({
        "id": "interactive_server"
    }).to_string();

    let diff_res = service.execute_tool("tmux_diff", &diff_args).await.unwrap();
    println!("Diff result: {}", diff_res);

    assert!(diff_res.contains("status"));
    assert!(diff_res.contains("recent_logs"));
    assert!(diff_res.contains("Enter password:")); // Should be in logs

    // Cleanup
    let kill_interactive = serde_json::json!({
        "id": "interactive_server",
        "action": "kill"
    }).to_string();
    service.execute_tool("tmux_control", &kill_interactive).await.unwrap();
}
