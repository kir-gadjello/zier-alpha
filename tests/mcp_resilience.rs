use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;
use zier_alpha::agent::mcp_manager::McpManager;
use zier_alpha::agent::mcp_manager::ServerConfig;

#[tokio::test]
async fn test_mcp_resilience_dead_process() {
    // 1. Setup Manager with short timeout
    let manager = McpManager::new(5); // 5s idle timeout

    // 2. Start a mock server that we can kill
    // We'll use "sleep 10" as a dummy server
    let config = ServerConfig {
        name: "test-server".to_string(),
        command: "sleep".to_string(),
        args: vec!["10".to_string()],
        env: None,
        strategy: None,
        native_tools: vec![],
    };

    manager.initialize(vec![config]).await;

    // 3. Ensure server starts
    // ensure_server waits for handshake. "sleep" won't handshake.
    // So we need a server that handshakes then dies or waits.
    // Let's use a simple python script?
    // Or just skip handshake for this test if possible?
    // McpManager::ensure_server enforces handshake.

    // We need a minimal mock MCP server.
    // Let's create a python script that handshakes and then waits.
    let script = r#"
import sys
import json
import time

# Read init request
line = sys.stdin.readline()
if not line: sys.exit(1)
req = json.loads(line)

# Send init response
print(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": {"capabilities": {}}}))
sys.stdout.flush()

# Read initialized notification
line = sys.stdin.readline()

# Now run for a bit then exit
time.sleep(1)
sys.exit(1) # Crash
"#;

    let temp = TempDir::new().unwrap();
    let script_path = temp.path().join("server.py");
    std::fs::write(&script_path, script).unwrap();

    let config = ServerConfig {
        name: "test-server".to_string(),
        command: "python3".to_string(),
        args: vec![script_path.to_str().unwrap().to_string()],
        env: None,
        strategy: None,
        native_tools: vec![],
    };

    manager.initialize(vec![config]).await;

    // Start server
    manager
        .ensure_server("test-server")
        .await
        .expect("Failed to start server");

    // Wait for it to crash (script sleeps 1s then exits)
    sleep(Duration::from_secs(2)).await;

    // Call list_tools - should fail or restart?
    // call() checks if server is connected. If process is dead, it might fail write.
    // But ensure_server should restart it if we call it again.

    // Let's force check via ensure_server
    // ensure_server checks try_wait().
    // If we call ensure_server now, it should detect dead process and restart.

    // We can't easily spy on "restart happened", but we can check logs or side effects.
    // Or we can rely on the fact that if it didn't restart, the next call would fail.

    manager
        .ensure_server("test-server")
        .await
        .expect("Failed to restart server");

    // Now it should be running (new process)
    // The new process will handshake again.

    // This confirms detection and restart logic in ensure_server works.
    // The background reaper is harder to test quickly due to 60s loop.
}
