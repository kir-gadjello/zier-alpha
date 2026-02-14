use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;
use std::path::PathBuf;
use crate::agent::session::{Session, SessionStatus};
use crate::config::Config;
use crate::agent::compaction::{CompactionStrategy, NativeCompactor};
use crate::agent::SmartClient;
use crate::agent::Role;
use crate::memory::MemoryManager;

/// Soft threshold buffer before compaction (tokens)
/// Memory flush runs when within this buffer of the hard limit
const MEMORY_FLUSH_SOFT_THRESHOLD: usize = 4000;

/// Generate a URL-safe slug from text (first 3-5 words, lowercased, hyphenated)
fn generate_slug(text: &str) -> String {
    text.split_whitespace()
        .take(4)
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(30)
        .collect()
}

#[derive(Clone)]
pub struct SessionManager {
    session: Arc<RwLock<Session>>,
    config: Config,
    compaction_strategy: Arc<dyn CompactionStrategy>,
}

impl SessionManager {
    pub fn new(config: Config) -> Self {
        Self {
            session: Arc::new(RwLock::new(Session::new())),
            config,
            compaction_strategy: Arc::new(NativeCompactor),
        }
    }

    pub fn session(&self) -> Arc<RwLock<Session>> {
        self.session.clone()
    }

    pub fn set_session(&mut self, session: Arc<RwLock<Session>>) {
        self.session = session;
    }

    pub fn set_compaction_strategy(&mut self, strategy: Arc<dyn CompactionStrategy>) {
        self.compaction_strategy = strategy;
    }

    pub async fn new_session(&self, _memory: &MemoryManager, _tools_prompt: Option<String>, system_prompt_fn: impl FnOnce() -> String) -> Result<()> {
        // Reset session
        {
            let mut session = self.session.write().await;
            *session = Session::new();
        }

        // Build and set system context
        let context = system_prompt_fn();
        self.session.write().await.set_system_context(context);

        info!("Created new session: {}", self.session.read().await.id());
        Ok(())
    }

    pub async fn load_session(&self, session_id: &str) -> Result<()> {
        let loaded = Session::load(session_id).await?;
        {
            let mut session = self.session.write().await;
            *session = loaded;
        }
        info!("Resumed session: {}", session_id);
        Ok(())
    }

    pub async fn hydrate_from_file(&self, path: &PathBuf) -> Result<()> {
        let current_id = self.session.read().await.id().to_string();
        let loaded = Session::load_file(path, &current_id).await?;
        {
            let mut session = self.session.write().await;
            *session = loaded;
        }
        info!("Hydrated session from {}", path.display());
        Ok(())
    }

    pub async fn save_session(&self) -> Result<PathBuf> {
        self.session.write().await.save().await
    }

    pub async fn save_session_for_agent(&self, agent_id: &str) -> Result<PathBuf> {
        self.session.write().await.save_for_agent(agent_id).await
    }

    pub async fn clear_session(&self) {
        let mut session = self.session.write().await;
        *session = Session::new();
    }

    pub async fn should_compact(&self, context_window: usize, reserve_tokens: usize) -> bool {
        let limit = context_window - reserve_tokens;
        self.compaction_strategy.should_compact(&*self.session.read().await, limit)
    }

    pub async fn should_memory_flush(&self, context_window: usize, reserve_tokens: usize) -> bool {
        let hard_limit = context_window - reserve_tokens;
        let soft_limit = hard_limit.saturating_sub(MEMORY_FLUSH_SOFT_THRESHOLD);

        let session = self.session.read().await;
        session.token_count() > soft_limit && session.should_memory_flush()
    }

    pub async fn mark_memory_flushed(&self) {
        self.session.write().await.mark_memory_flushed();
    }

    pub async fn compact_session(&self, client: &SmartClient) -> Result<(usize, usize)> {
        let before = self.session.read().await.token_count();

        // Compact the session
        {
            let mut session = self.session.write().await;

            // Try primary strategy first
            match self.compaction_strategy.compact(&mut *session, client).await {
                Ok(_) => {},
                Err(e) => {
                    info!("Compaction failed with primary model: {}", e);

                    // Try fallback models if primary fails
                    let mut fallback_success = false;

                    // Check strategy setting
                    let strategy = &self.config.agent.compaction.strategy;
                    let try_models = strategy != "truncate";

                    if try_models {
                        for model in &self.config.agent.compaction.fallback_models {
                            info!("Retrying compaction with fallback model: {}", model);

                            let fallback_client = SmartClient::new(self.config.clone(), model.clone());

                            match self.compaction_strategy.compact(&mut *session, &fallback_client).await {
                                Ok(_) => {
                                    fallback_success = true;
                                    info!("Fallback compaction succeeded with {}", model);
                                    break;
                                },
                                Err(e_fallback) => {
                                    tracing::warn!("Fallback compaction with {} failed: {}", model, e_fallback);
                                }
                            }
                        }
                    }

                    if !fallback_success {
                        // If all models failed (or skipped), try truncation if allowed
                        // "models_then_truncate" or "truncate" implies truncation is last resort
                        if strategy == "truncate" || strategy == "models_then_truncate" {
                            info!("Compaction models failed, falling back to truncation.");
                            self.compact_truncate(&mut *session);
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
        }

        let after = self.session.read().await.token_count();
        info!("Session compacted: {} -> {} tokens", before, after);

        Ok((before, after))
    }

    fn compact_truncate(&self, session: &mut Session) {
        let keep = self.config.agent.compaction.keep_last;
        session.truncate_history(keep);
        info!("Truncated session history to last {} messages", keep);
    }

    pub async fn save_session_to_memory(&self, memory: &MemoryManager) -> Result<Option<PathBuf>> {
        let messages = self.session.read().await.user_assistant_messages();

        if messages.is_empty() {
            return Ok(None);
        }

        let max_messages = self.config.memory.session_max_messages;
        let max_chars = self.config.memory.session_max_chars;

        let messages: Vec<_> = if max_messages > 0 && messages.len() > max_messages {
            let skip_count = messages.len() - max_messages;
            messages.into_iter().skip(skip_count).collect()
        } else {
            messages
        };

        let slug = messages
            .iter()
            .find(|m| m.role == Role::User)
            .map(|m| generate_slug(&m.content))
            .unwrap_or_else(|| "session".to_string());

        let now = chrono::Local::now();
        let date_str = now.format("%Y-%m-%d").to_string();
        let time_str = now.format("%H:%M:%S").to_string();

        let mut content = format!(
            "# Session: {} {}\n\n\
             - **Session ID**: {}\n\n\
             ## Conversation\n\n",
            date_str,
            time_str,
            self.session.read().await.id()
        );

        for msg in &messages {
            let role = match msg.role {
                Role::User => "**User**",
                Role::Assistant => "**Assistant**",
                _ => continue,
            };
            let (msg_content, truncated) =
                if max_chars > 0 && msg.content.chars().count() > max_chars {
                    (
                        msg.content.chars().take(max_chars).collect::<String>(),
                        "...",
                    )
                } else {
                    (msg.content.clone(), "")
                };
            content.push_str(&format!("{}: {}{}\n\n", role, msg_content, truncated));
        }

        let memory_dir = memory.workspace().join("memory");
        tokio::fs::create_dir_all(&memory_dir).await?;

        let filename = format!("{}-{}.md", date_str, slug);
        let path = memory_dir.join(&filename);

        tokio::fs::write(&path, content).await?;
        info!("Saved session to memory: {}", path.display());

        Ok(Some(path))
    }

    pub async fn session_status(&self, cumulative_input: u64, cumulative_output: u64) -> SessionStatus {
        self.session.read().await.status_with_usage(
            cumulative_input,
            cumulative_output,
        )
    }

    pub async fn auto_save_session(&self) -> Result<()> {
        self.session.write().await.auto_save().await
    }
}
