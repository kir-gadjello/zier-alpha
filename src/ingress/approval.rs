use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{sleep_until, timeout_at, Instant};
use uuid::Uuid;

/// Decision returned by an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}

/// Request to send an approval UI message to the Telegram service.
#[derive(Debug)]
pub struct ApprovalUIRequest {
    pub call_id: String,
    pub chat_id: i64,
    pub tool_name: String,
    pub arguments: String,
    /// Respond with the Telegram message_id of the UI message.
    pub respond_msg_id: oneshot::Sender<i64>,
}

/// A pending approval request waiting for user interaction.
#[derive(Debug)]
struct PendingApproval {
    chat_id: i64,
    message_id: i64,
    tool_name: String,
    arguments: String,
    tx: oneshot::Sender<ApprovalDecision>,
    timeout_at: Instant,
}

/// Coordinator for managing approval requests from the agent to the UI.
/// Holds a channel to send UI requests to the Telegram service.
#[derive(Debug, Clone)]
pub struct ApprovalCoordinator {
    pending: Arc<Mutex<HashMap<String, PendingApproval>>>,
    ui_tx: mpsc::Sender<ApprovalUIRequest>,
}

impl ApprovalCoordinator {
    /// Create a new ApprovalCoordinator with a sender for UI requests.
    pub fn new(ui_tx: mpsc::Sender<ApprovalUIRequest>) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            ui_tx,
        }
    }

    /// Register a new approval request and wait for a decision.
    /// This sends a UI request to the Telegram service, then waits
    /// for the user to approve/deny or for timeout.
    /// Returns `Some(decision)` if resolved, `None` on timeout or error.
    pub async fn request(
        &self,
        call_id: String,
        chat_id: i64,
        tool: String,
        args: String,
        timeout: std::time::Duration,
    ) -> Option<ApprovalDecision> {
        // Channel to receive the UI message_id after it's sent.
        let (tx_msg_id, rx_msg_id) = oneshot::channel();

        // Build UI request
        let ui_req = ApprovalUIRequest {
            call_id: call_id.clone(),
            chat_id,
            tool_name: tool.clone(),
            arguments: args.clone(),
            respond_msg_id: tx_msg_id,
        };

        // Send UI request to Telegram service
        if self.ui_tx.send(ui_req).await.is_err() {
            // UI service gone
            return None;
        }

        // Wait for the message_id with overall timeout.
        let deadline = Instant::now() + timeout;
        let msg_id = match timeout_at(deadline, rx_msg_id).await {
            Ok(Ok(id)) => id,
            _ => {
                // UI response timed out or channel closed
                return None;
            }
        };

        // Now create pending entry with the received message_id.
        let (tx_decision, rx_decision) = oneshot::channel();
        let entry = PendingApproval {
            chat_id,
            message_id: msg_id,
            tool_name: tool,
            arguments: args,
            tx: tx_decision,
            timeout_at: deadline,
        };

        {
            let mut map = self.pending.lock().await;
            map.insert(call_id.clone(), entry);
        }

        // Wait for decision (with remaining time)
        tokio::select! {
            res = rx_decision => res.ok(),
            _ = sleep_until(deadline) => {
                // Timeout: remove entry
                let mut map = self.pending.lock().await;
                map.remove(&call_id);
                None
            }
        }
    }

    /// Resolve a pending approval by call_id with the given decision.
    /// Called by the Telegram service when user clicks a button.
    /// Returns the chat_id and message_id for UI update, or None if not found.
    pub async fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> Option<(i64, i64)> {
        let mut map = self.pending.lock().await;
        if let Some(entry) = map.remove(call_id) {
            // Send the decision; if the receiver is gone, ignore.
            let _ = entry.tx.send(decision);
            Some((entry.chat_id, entry.message_id))
        } else {
            None
        }
    }

    /// Clean up any pending approvals that have timed out (i.e., timeout_at <= now).
    /// Returns a vector of (call_id, chat_id, message_id) for each expired entry
    /// that hasn't been resolved yet. The caller can use this to edit the UI messages.
    pub async fn cleanup(&self, now: Instant) -> Vec<(String, i64, i64)> {
        let mut map = self.pending.lock().await;
        let mut expired = Vec::new();

        // Collect call_ids to remove
        let mut to_remove = Vec::new();
        for (call_id, entry) in map.iter() {
            if entry.timeout_at <= now {
                to_remove.push(call_id.clone());
            }
        }

        // Remove and gather info
        for call_id in to_remove {
            if let Some(entry) = map.remove(&call_id) {
                expired.push((call_id, entry.chat_id, entry.message_id));
            }
        }

        expired
    }
}
