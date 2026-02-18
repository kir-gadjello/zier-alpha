// Integration test for Telegram audio transcription.
// Verifies that voice/audio messages are transcribed and, if transcription fails, fallback to attachment.

use std::path::PathBuf;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::fs;
use zier_alpha::agent::Agent;
use zier_alpha::agent::AgentConfig;
use zier_alpha::agent::ContextStrategy;
use zier_alpha::config::{Config, MemoryConfig};
use zier_alpha::ingress::approval::ApprovalCoordinator;
use zier_alpha::ingress::TelegramApi;
use zier_alpha::ingress::{IngressBus, TelegramMessage as TelegramMsg, TrustLevel};
use zier_alpha::memory::MemoryManager;
use zier_alpha::server::telegram_polling::TelegramPollingService;

// Mock Telegram client
#[derive(Clone)]
struct MockTelegramClient {
    file_content: Vec<u8>,
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
        _offset: Option<i64>,
        _timeout: u64,
    ) -> anyhow::Result<Vec<zier_alpha::ingress::TelegramUpdate>> {
        Ok(vec![])
    }
    async fn send_message(&self, _chat_id: i64, _text: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn edit_message_text(
        &self,
        _chat_id: i64,
        _message_id: i64,
        _text: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn answer_callback_query(
        &self,
        _query_id: &str,
        _text: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    async fn get_file_download_url(&self, file_id: &str) -> anyhow::Result<String> {
        Ok(format!("http://test/download/{}", file_id))
    }
    async fn download_file(&self, _url: &str) -> anyhow::Result<Vec<u8>> {
        Ok(self.file_content.clone())
    }
    async fn send_approval_message(
        &self,
        _chat_id: i64,
        _text: &str,
        _call_id: &str,
    ) -> anyhow::Result<i64> {
        Ok(999)
    }
}

#[tokio::test]
async fn test_audio_transcription_flow() {
    // 1. Setup
    let temp_dir = tempdir().unwrap();
    let project_dir = temp_dir.path().to_path_buf();
    let workspace_dir = project_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).await.unwrap();

    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().into_owned();
    config.server.telegram_bot_token = Some("test_token".to_string());
    config.server.owner_telegram_id = Some(123456789_i64);
    config.agent.default_model = "mock/test".to_string();

    // Enable audio transcription with a local command that echoes a fixed string.
    config.server.audio.enabled = true;
    config.server.audio.backend = "local".to_string();
    config.server.audio.local_command = Some("echo \"TRANSCRIPTED TEXT\"".to_string());

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent")
        .expect("MemoryManager failed");
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 128000,
        reserve_tokens: 8000,
    };
    let _agent = Arc::new(tokio::sync::Mutex::new(
        Agent::new_with_project(
            agent_config,
            &config,
            memory,
            ContextStrategy::Stateless,
            project_dir.clone(),
            "test-agent",
        )
        .await
        .expect("Agent creation failed"),
    ));

    let bus = Arc::new(IngressBus::new(100));

    // Mock client returns dummy audio bytes (content not used by transcriber)
    let mock_client = Arc::new(MockTelegramClient::new(b"fake audio data".to_vec()));

    let (approval_ui_tx, _approval_ui_rx) = tokio::sync::mpsc::channel(100);
    let approval_coord = Arc::new(ApprovalCoordinator::new(approval_ui_tx));

    let service = TelegramPollingService::new(
        config.clone(),
        bus.clone(),
        project_dir.clone(),
        approval_coord,
        tokio::sync::mpsc::channel(1).1,
        Some(mock_client),
    )
    .expect("Service creation failed");

    // Fake voice message
    let fake_message = TelegramMsg {
        message_id: 200,
        from: Some(zier_alpha::ingress::TelegramUser { id: 123456789_i64 }),
        text: None,
        photo: None,
        document: None,
        audio: None,
        voice: Some(zier_alpha::ingress::TelegramVoice {
            file_id: "voice123".to_string(),
            mime_type: Some("audio/ogg".to_string()),
            file_size: Some(1234),
            duration: Some(5),
        }),
        caption: Some("Voice note caption".to_string()),
    };

    // Process
    service
        .process_message_for_test(fake_message)
        .await
        .unwrap();

    // Verify that a message was pushed to bus with expected transcript
    let receiver_arc = bus.receiver().clone();
    let mut receiver = receiver_arc.lock().await;
    let received = tokio::time::timeout(tokio::time::Duration::from_secs(1), receiver.recv())
        .await
        .unwrap()
        .expect("No message received on bus");

    assert!(received.source.starts_with("telegram:"));
    let payload = &received.payload;
    // The transcript should be "TRANSCRIPTED TEXT" possibly with caption
    assert!(
        payload.contains("TRANSCRIPTED TEXT"),
        "Payload missing transcript: {}",
        payload
    );
    // Caption should be included
    assert!(
        payload.contains("Voice note caption"),
        "Payload missing caption: {}",
        payload
    );
}

#[tokio::test]
async fn test_audio_fallback_to_attachment_when_no_transcriber() {
    // Similar setup but with transcriber disabled (config.server.audio.enabled = false or missing)
    // Expect that the voice message is handled as an attachment (XML in payload).
    let temp_dir = tempdir().unwrap();
    let project_dir = temp_dir.path().to_path_buf();
    let workspace_dir = project_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).await.unwrap();

    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().into_owned();
    config.server.telegram_bot_token = Some("test_token".to_string());
    config.server.owner_telegram_id = Some(123456789_i64);
    config.agent.default_model = "mock/test".to_string();
    // Audio disabled
    config.server.audio.enabled = false;

    let memory =
        MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent").unwrap();
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 128000,
        reserve_tokens: 8000,
    };
    let _agent = Arc::new(tokio::sync::Mutex::new(
        Agent::new_with_project(
            agent_config,
            &config,
            memory,
            ContextStrategy::Stateless,
            project_dir.clone(),
            "test-agent",
        )
        .await
        .unwrap(),
    ));

    let bus = Arc::new(IngressBus::new(100));
    let mock_client = Arc::new(MockTelegramClient::new(b"fake audio".to_vec()));
    let (approval_ui_tx, _approval_ui_rx) = tokio::sync::mpsc::channel(100);
    let approval_coord = Arc::new(ApprovalCoordinator::new(approval_ui_tx));

    let service = TelegramPollingService::new(
        config.clone(),
        bus.clone(),
        project_dir.clone(),
        approval_coord,
        tokio::sync::mpsc::channel(1).1,
        Some(mock_client),
    )
    .unwrap();

    let fake_message = TelegramMsg {
        message_id: 201,
        from: Some(zier_alpha::ingress::TelegramUser { id: 123456789_i64 }),
        text: None,
        photo: None,
        document: None,
        audio: None,
        voice: Some(zier_alpha::ingress::TelegramVoice {
            file_id: "voice456".to_string(),
            mime_type: Some("audio/ogg".to_string()),
            file_size: Some(5678),
            duration: Some(10),
        }),
        caption: Some("Fallback attachment".to_string()),
    };

    service
        .process_message_for_test(fake_message)
        .await
        .unwrap();

    // Since no transcriber, it should fall back to attachment handling -> file saved + XML block.
    // Check file exists: path expected attachments/telegram/201_123456789_? Since we didn't provide filename, it will be "file"
    let expected_path = project_dir
        .join("attachments")
        .join("telegram")
        .join("201_123456789_file");
    assert!(
        expected_path.exists(),
        "Attachment file not found at {:?}",
        expected_path
    );

    // Check bus message
    let receiver_arc = bus.receiver().clone();
    let mut receiver = receiver_arc.lock().await;
    let received = tokio::time::timeout(tokio::time::Duration::from_secs(1), receiver.recv())
        .await
        .unwrap()
        .expect("No message on bus");

    let payload = &received.payload;
    assert!(
        payload.contains("Fallback attachment"),
        "Caption missing: {}",
        payload
    );
    assert!(
        payload.contains(r#"path="attachments/telegram/201_123456789_file""#),
        "XML path missing: {}",
        payload
    );
}

#[tokio::test]
async fn test_audio_transcription_error_fallback_to_attachment() {
    // Test that if transcription fails (e.g., API error), the system falls back to saving the audio as attachment.
    let temp_dir = tempdir().unwrap();
    let project_dir = temp_dir.path().to_path_buf();
    let workspace_dir = project_dir.join("workspace");
    fs::create_dir_all(&workspace_dir).await.unwrap();

    let mut config = Config::default();
    config.memory.workspace = workspace_dir.to_string_lossy().into_owned();
    config.server.telegram_bot_token = Some("test_token".to_string());
    config.server.owner_telegram_id = Some(123456789_i64);
    config.agent.default_model = "mock/test".to_string();

    // Enable audio with local command that will fail (non-zero exit)
    config.server.audio.enabled = true;
    config.server.audio.backend = "local".to_string();
    config.server.audio.local_command = Some("false".to_string()); // command that fails

    let memory =
        MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent").unwrap();
    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 128000,
        reserve_tokens: 8000,
    };
    let _agent = Arc::new(tokio::sync::Mutex::new(
        Agent::new_with_project(
            agent_config,
            &config,
            memory,
            ContextStrategy::Stateless,
            project_dir.clone(),
            "test-agent",
        )
        .await
        .unwrap(),
    ));

    let bus = Arc::new(IngressBus::new(100));
    // Mock client returns dummy audio bytes
    let mock_client = Arc::new(MockTelegramClient::new(
        b"fake audio data that fails".to_vec(),
    ));

    let (approval_ui_tx, _approval_ui_rx) = tokio::sync::mpsc::channel(100);
    let approval_coord = Arc::new(ApprovalCoordinator::new(approval_ui_tx));

    let service = TelegramPollingService::new(
        config.clone(),
        bus.clone(),
        project_dir.clone(),
        approval_coord,
        tokio::sync::mpsc::channel(1).1,
        Some(mock_client),
    )
    .unwrap();

    let fake_message = TelegramMsg {
        message_id: 202,
        from: Some(zier_alpha::ingress::TelegramUser { id: 123456789_i64 }),
        text: None,
        photo: None,
        document: None,
        audio: None,
        voice: Some(zier_alpha::ingress::TelegramVoice {
            file_id: "voice202".to_string(),
            mime_type: Some("audio/ogg".to_string()),
            file_size: Some(1234),
            duration: Some(5),
        }),
        caption: Some("Error fallback test".to_string()),
    };

    service
        .process_message_for_test(fake_message)
        .await
        .unwrap();

    // Because transcription fails, expect fallback to attachment: file saved and XML present.
    let expected_path = project_dir
        .join("attachments")
        .join("telegram")
        .join("202_123456789_file");
    assert!(
        expected_path.exists(),
        "Fallback attachment file not found at {:?}",
        expected_path
    );

    // Check bus message contains XML with path and caption
    let receiver_arc = bus.receiver().clone();
    let mut receiver = receiver_arc.lock().await;
    let received = tokio::time::timeout(tokio::time::Duration::from_secs(1), receiver.recv())
        .await
        .unwrap()
        .expect("No message on bus");

    let payload = &received.payload;
    assert!(
        payload.contains("Error fallback test"),
        "Caption missing in fallback: {}",
        payload
    );
    assert!(
        payload.contains(r#"path="attachments/telegram/202_123456789_file""#),
        "XML path missing in fallback: {}",
        payload
    );
}
