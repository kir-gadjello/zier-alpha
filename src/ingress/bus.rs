use super::types::IngressMessage;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Global event bus for the Kernel.
/// Decouples Ingress from Execution.
pub struct IngressBus {
    sender: Sender<IngressMessage>,
    receiver: Arc<Mutex<Receiver<IngressMessage>>>,
}

impl IngressBus {
    /// Create a new bus with a channel capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender,
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }

    /// Get a sender handle for external providers.
    pub fn sender(&self) -> Sender<IngressMessage> {
        self.sender.clone()
    }

    /// Get the receiver handle (only one consumer allowed/expected for now, but wrapped in Arc<Mutex> for flexibility).
    pub fn receiver(&self) -> Arc<Mutex<Receiver<IngressMessage>>> {
        self.receiver.clone()
    }

    /// Send a message to the bus.
    pub async fn push(&self, msg: IngressMessage) -> Result<()> {
        self.sender.send(msg).await.map_err(|e| anyhow::anyhow!("Failed to push to IngressBus: {}", e))
    }
}

/// Trait for external listeners (Telegram, Webhook, Cron) to push to the bus.
#[async_trait]
pub trait IngressProvider: Send + Sync {
    /// Start listening and pushing messages to the bus.
    async fn run(&self, bus: Sender<IngressMessage>) -> Result<()>;
}
