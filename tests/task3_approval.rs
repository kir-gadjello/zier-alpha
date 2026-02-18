use anyhow::Result;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zier_alpha::ingress::approval::{ApprovalCoordinator, ApprovalDecision, ApprovalUIRequest};

#[tokio::test]
async fn test_approval_coordinator_race() -> Result<()> {
    // 1. Setup coordinator
    let (ui_tx, mut ui_rx) = mpsc::channel::<ApprovalUIRequest>(1);
    let coord = ApprovalCoordinator::new(ui_tx);

    let coord_clone = coord.clone();
    let call_id = "call_123".to_string();
    let call_id_clone = call_id.clone();

    // 2. Spawn request task
    let request_handle = tokio::spawn(async move {
        coord_clone.request(
            call_id_clone,
            100, // chat_id
            "test_tool".to_string(),
            "{}".to_string(),
            Duration::from_secs(5),
        ).await
    });

    // 3. Receive UI request
    // This blocks until `request` calls `ui_tx.send`.
    let req = ui_rx.recv().await.expect("Should receive UI request");

    // At this point, in the ORIGINAL code, `request` is blocked on `rx_msg_id` OR sending.
    // In original code, `insert` happens AFTER `rx_msg_id` resolves.
    // So if we resolve NOW, `resolve` will fail to find entry (race condition).

    // 4. Resolve immediately (simulate user clicking button before message_id is returned)
    // We expect this to SUCCEED in the fixed version.
    // In the buggy version, this returns None because map insert hasn't happened yet (it waits for msg_id).

    // We delay slightly to ensure `request` task reaches `rx_msg_id` await?
    // Actually, `ui_tx.send` is awaited. So `request` task is at `timeout_at(..., rx_msg_id).await`.
    // It has NOT inserted into map yet.

    let resolve_res = coord.resolve(&call_id, ApprovalDecision::Approve).await;

    // In buggy version: resolve_res is likely None.
    // In fixed version: resolve_res should be Some((100, -1)).

    // 5. Send message_id (simulating Telegram API response)
    let _ = req.respond_msg_id.send(999);

    // 6. Await result
    let result = request_handle.await?;

    assert_eq!(result, Some(ApprovalDecision::Approve));

    // Verify resolve result (if we fix it)
    // assert!(resolve_res.is_some());

    Ok(())
}
