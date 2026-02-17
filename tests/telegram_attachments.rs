// Integration test for Telegram attachment downloads and XML injection.
// Verifies that documents are saved to disk and an XML block is injected into the agent's message.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::fs;
use zier_alpha::agent::Agent;
use zier_alpha::agent::AgentConfig;
use zier_alpha::agent::ContextStrategy;
use zier_alpha::config::{Config, MemoryConfig};
use zier_alpha::ingress::approval::ApprovalCoordinator;
use zier_alpha::ingress::{IngressBus, TelegramApi, TelegramMessage as TelegramMsg, TrustLevel};
use zier_alpha::memory::MemoryManager;
use zier_alpha::server::telegram_polling::TelegramPollingService;

// Mock Telegram client for tests
#[derive(Clone)]
struct MockTelegramClient {
    file_content: Vec<u8>,
    // optionally record calls
}

impl MockTelegramClient {
    fn new(file_content: Vec<u8>) -> Self {
        Self { file_content }
    }
}

#[async_trait::async_trait]
impl TelegramApi for MockTelegramClient {
    async fn get_updates(
        &self,
        offset: Option<i64>,
        timeout: u64,
    ) -> anyhow::Result<Vec<zier_alpha::ingress::TelegramUpdate>> {
        // Not used in this test
        Ok(vec![])
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        // Not used
        Ok(())
    }

    async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn answer_callback_query(
        &self,
        query_id: &str,
        text: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn get_file_download_url(&self, file_id: &str) -> anyhow::Result<String> {
        // Return a dummy URL
        Ok(format!("http://test/download/{}", file_id))
    }

    async fn download_file(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        // Simulate downloading the pre‑set file content
        Ok(self.file_content.clone())
    }

    async fn send_approval_message(
        &self,
        chat_id: i64,
        text: &str,
        call_id: &str,
    ) -> anyhow::Result<i64> {
        // Not used in this test; return a dummy message_id
        Ok(999)
    }
}

#[tokio::test]
async fn test_attachment_download_and_injection() {
    // 1. Setup temp directories
    let temp_dir = tempdir().unwrap();
    let project_dir = temp_dir.path().to_path_buf();
    let workspace_dir = project_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).await.unwrap();

    // 2. Config with attachments enabled
    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().into_owned();
    config.server.attachments.enabled = true;
    config.server.attachments.max_file_size_bytes = 10_000_000;
    config.server.attachments.base_dir = "attachments".to_string();
    config.server.telegram_bot_token = Some("test_token".to_string());
    config.server.owner_telegram_id = Some(123456789_i64);
    config.agent.default_model = "mock/test".to_string();

    // 3. Memory and Agent (we only need agent for session, not actually used)
    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent")
        .expect("Failed to create memory manager");
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 128000,
        reserve_tokens: 8000,
    };
    let agent = Arc::new(tokio::sync::Mutex::new(
        Agent::new_with_project(
            agent_config,
            &config,
            memory,
            ContextStrategy::Stateless,
            project_dir.clone(),
            "test-agent",
        )
        .await
        .expect("Failed to create agent"),
    ));

    // 4. Ingress bus
    let bus = Arc::new(IngressBus::new(100));

    // 5. Mock client with known content
    let file_content = b"This is a test document.";
    let mock_client = Arc::new(MockTelegramClient::new(file_content.to_vec()));

    // 6. Approval coordinator (dummy)
    let (approval_ui_tx, _approval_ui_rx) = tokio::sync::mpsc::channel(100);
    let approval_coord = Arc::new(ApprovalCoordinator::new(approval_ui_tx));

    // 7. Build TelegramPollingService
    let service = TelegramPollingService::new(
        config.clone(),
        bus.clone(),
        project_dir.clone(),
        approval_coord,
        // We need a receiver for approval_ui_rx but not used in this test; provide empty channel
        tokio::sync::mpsc::channel(1).1,
        Some(mock_client),
    )
    .expect("Failed to create service");

    // 8. Construct a fake TelegramMessage with a document
    let fake_message = TelegramMsg {
        message_id: 100,
        from: Some(zier_alpha::ingress::TelegramUser { id: 123456789_i64 }),
        text: None,
        photo: None,
        document: Some(zier_alpha::ingress::TelegramDocument {
            file_id: "doc123".to_string(),
            file_name: Some("test.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
            file_size: Some(file_content.len() as i64),
        }),
        audio: None,
        voice: None,
        caption: Some("Here is the file".to_string()),
    };

    // 9. Process the message
    service
        .process_message_for_test(fake_message)
        .await
        .unwrap();

    // 10. Check that file was saved
    let expected_path = project_dir
        .join("attachments")
        .join("telegram")
        .join("100_123456789_test.txt");
    assert!(
        expected_path.exists(),
        "Attachment file not found at {}",
        expected_path.display()
    );
    let saved_content = fs::read(&expected_path).await.unwrap();
    assert_eq!(saved_content, file_content);

    // 11. Check that a message was pushed to the bus containing the XML block
    // The bus receiver: we need to get the message. Since the service pushed one message,
    // we can try to receive it with a timeout.
    let receiver_arc = bus.receiver();
    let mut receiver = receiver_arc.lock().await;
    let received = tokio::time::timeout(tokio::time::Duration::from_secs(1), receiver.recv())
        .await
        .unwrap()
        .expect("No message received on bus");

    // The message source should be "telegram:123456789"
    assert!(received.source.starts_with("telegram:"));
    // The payload should contain the caption and the XML with the relative path
    assert!(
        received.payload.contains("Here is the file"),
        "Payload missing caption"
    );
    assert!(
        received
            .payload
            .contains(r#"path="attachments/telegram/100_123456789_test.txt""#),
        "Payload missing XML path reference. Payload: {}",
        received.payload
    );
    assert!(
        received.payload.contains(r#"filename="test.txt""#),
        "Payload missing filename attribute. Payload: {}",
        received.payload
    );
}
