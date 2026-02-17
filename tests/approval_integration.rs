// Integration test for tool approvals via ApprovalCoordinator in the ingress loop.
// Uses mock provider to simulate tool call requiring approval.

use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;
use zier_alpha::config::{Config, SandboxPolicy, WorkdirStrategy};
use zier_alpha::ingress::{
    controller::ingress_loop, ApprovalCoordinator, IngressBus, IngressMessage, TrustLevel,
};
use zier_alpha::memory::MemoryManager;
use zier_alpha::prompts::PromptRegistry;
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_approval_integration() {
    // 1. Setup temporary workspace
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    // 2. Build config
    let mut config = Config::default();
    config.memory.workspace = workspace_path.to_string_lossy().into_owned();
    config.agent.default_model = "mock/test".to_string();
    config.tools.require_approval = vec!["bash".to_string()];
    config.tools.allowed_builtin = vec!["*".to_string()];
    // Set a dummy Telegram token so that ingress loop doesn't try to use a real client.
    config.server.telegram_bot_token = Some("dummy_token".to_string());

    // 3. Create MemoryManager
    let memory =
        MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent").unwrap();

    // 4. Create ScriptService (minimal)
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![workspace_path.to_str().unwrap().to_string()],
        allow_write: vec![workspace_path.to_str().unwrap().to_string()],
        allow_env: false,
        enable_os_sandbox: false,
    };
    let script_service = ScriptService::new(
        policy,
        workspace_path.clone(),
        workspace_path.clone(),
        WorkdirStrategy::Overlay,
        None,
        None,
        None,
        "test-agent".to_string(),
    )
    .unwrap();

    // 5. PromptRegistry (empty)
    let prompts = Arc::new(PromptRegistry::new());

    // 6. Ingress Bus
    let bus = Arc::new(IngressBus::new(100));

    // 7. Scheduler jobs (empty)
    let jobs = vec![];

    // 8. Create channel for approval UI requests
    let (approval_ui_tx, mut approval_ui_rx) = mpsc::channel(100);
    let approval_coord = Arc::new(ApprovalCoordinator::new(approval_ui_tx));

    // 9. Prepare controller inputs
    let receiver = bus.receiver();
    let config_clone = config.clone();
    let agent_id = "test-agent".to_string();
    let prompts_clone = prompts.clone();
    let script_service_clone = script_service.clone();
    let approval_coord_clone = approval_coord.clone();

    // 10. Spawn ingress_loop
    let handler_handle = tokio::spawn(async move {
        ingress_loop(
            receiver,
            config_clone,
            agent_id,
            prompts_clone,
            script_service_clone,
            jobs,
            approval_coord_clone,
        )
        .await;
    });

    // 11. Give the loop a moment to initialize
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // 12. Send an OwnerCommand message that will trigger a tool call for 'bash'.
    // Using the mock provider: sending "test_tool_json:bash|{\"cmd\":\"echo hi\"}" causes a ToolCall.
    let msg = IngressMessage::new(
        "telegram:123456".to_string(),
        "test_tool_json:bash|{\"cmd\":\"echo hi\"}".to_string(),
        TrustLevel::OwnerCommand,
    );
    bus.push(msg).await.unwrap();

    // 13. The ingress_loop will process the message. Because the mock provider returns a ToolCall for bash,
    // and bash is in require_approval, the process_ingress_message should call approval_coord.request,
    // which sends an ApprovalUIRequest to the approval_ui_tx channel.

    // 14. Wait to receive the ApprovalUIRequest from the channel
    let ui_req = approval_ui_rx
        .recv()
        .await
        .expect("Expected ApprovalUIRequest not received");

    assert_eq!(ui_req.tool_name, "bash");
    assert_eq!(ui_req.chat_id, 123456);
    let call_id = ui_req.call_id.clone();

    // 15. Simulate the Telegram service handling the UI request: we need to send the message_id back via the oneshot.
    // In the real flow, handle_approval_ui_request would call send_approval_message and then send the message_id.
    // We'll manually send a dummy message_id to unblock the coordinator's request.
    // The oneshot sender is in ui_req.respond_msg_id.
    let _ = ui_req.respond_msg_id.send(555); // dummy message_id

    // 16. Wait a short time for the coordinator to register the pending request before resolving.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // 17. Now resolve the approval: approve the tool call.
    approval_coord
        .resolve(&call_id, zier_alpha::ingress::ApprovalDecision::Approve)
        .await
        .expect("resolve should succeed");

    // 17. After approval, the agent should continue and produce a final response.
    // We don't need to check the response artifact; the key is that the flow completes without deadlock.

    // Wait a bit for processing
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // 18. Stop the ingress_loop by dropping the bus (which closes the channel)
    drop(bus);

    // 19. Wait for the handler task to exit cleanly
    use tokio::time::timeout;
    let res = timeout(tokio::time::Duration::from_secs(2), handler_handle).await;
    assert!(
        res.is_ok(),
        "ingress_loop should exit cleanly after bus drop"
    );
    let _ = res.unwrap(); // Ok(()) is expected
}
