use crate::agent::providers::LLMProvider;
use crate::agent::session::Session;
use crate::scripting::ScriptService;
use anyhow::Result;
use async_trait::async_trait;
use tiktoken_rs::cl100k_base;
use serde_json::json;
use tracing::warn;

#[async_trait]
pub trait CompactionStrategy: Send + Sync {
    async fn compact(&self, session: &mut Session, provider: &dyn LLMProvider) -> Result<()>;
    fn should_compact(&self, session: &Session, limit: usize) -> bool;
}

pub struct NativeCompactor;

#[async_trait]
impl CompactionStrategy for NativeCompactor {
    async fn compact(&self, session: &mut Session, provider: &dyn LLMProvider) -> Result<()> {
        session.compact_native(provider).await
    }

    fn should_compact(&self, session: &Session, limit: usize) -> bool {
        let bpe = match cl100k_base() {
            Ok(b) => b,
            Err(_) => return session.token_count() > limit,
        };

        let mut count = 0;
        if let Some(ctx) = session.system_context() {
            count += bpe.encode_with_special_tokens(ctx).len();
        }

        for msg in session.messages() {
            count += bpe.encode_with_special_tokens(&msg.content).len();
        }

        count > limit
    }
}

pub struct ScriptCompactor {
    service: ScriptService,
    script_path: String,
}

impl ScriptCompactor {
    pub fn new(service: ScriptService, script_path: String) -> Self {
        Self { service, script_path }
    }
}

#[async_trait]
impl CompactionStrategy for ScriptCompactor {
    async fn compact(&self, session: &mut Session, _provider: &dyn LLMProvider) -> Result<()> {
        self.service.load_script(&self.script_path).await?;

        let messages = session.messages();
        let args = json!({
            "messages": messages,
            "system_context": session.system_context()
        });

        let result_json = self.service.execute_tool("compact", &args.to_string()).await?;

        // Update session with results
        if let Err(e) = session.replace_messages_from_json(&result_json) {
            warn!("Failed to parse script compaction result: {}", e);
            anyhow::bail!("Script compaction failed parsing: {}", e);
        }

        Ok(())
    }

    fn should_compact(&self, session: &Session, limit: usize) -> bool {
        NativeCompactor.should_compact(session, limit)
    }
}
