pub mod chat_engine;
pub mod client;
pub mod compaction;
pub mod llm_error;
pub mod mcp_manager;
pub mod memory_context;
pub mod providers;
pub mod sanitize;
pub mod session;
pub mod session_manager;
pub mod session_store;
pub mod skills;
pub mod system_prompt;
pub mod tool_executor;
pub mod tools;

pub use chat_engine::ChatEngine;
pub use llm_error::LlmError;
pub use mcp_manager::McpManager;
pub use memory_context::MemoryContextBuilder;
pub use providers::{
    ImageAttachment, LLMProvider, LLMResponse, LLMResponseContent, Message, Role, StreamChunk,
    StreamEvent, StreamResult, ToolCall, ToolSchema, Usage,
};
pub use sanitize::{
    wrap_external_content, wrap_memory_content, wrap_tool_output, MemorySource, SanitizeResult,
    EXTERNAL_CONTENT_END, EXTERNAL_CONTENT_START, MEMORY_CONTENT_END, MEMORY_CONTENT_START,
    TOOL_OUTPUT_END, TOOL_OUTPUT_START,
};
pub use session::{
    get_last_session_id, get_last_session_id_for_agent, get_sessions_dir_for_agent, get_state_dir,
    list_sessions, list_sessions_for_agent, search_sessions, search_sessions_for_agent, Session,
    SessionInfo, SessionMessage, SessionSearchResult, SessionStatus, DEFAULT_AGENT_ID,
};
pub use session_manager::SessionManager;
pub use session_store::{SessionEntry, SessionStore};
pub use skills::{get_skills_summary, load_skills, parse_skill_command, Skill, SkillInvocation};
pub use system_prompt::{
    build_heartbeat_prompt, is_heartbeat_ok, is_silent_reply, SystemPromptContext,
    HEARTBEAT_OK_TOKEN, SILENT_REPLY_TOKEN,
};
pub use tool_executor::ToolExecutor;
pub use tools::{create_default_tools, extract_tool_detail, ScriptTool, Tool, ToolResult};
pub mod disk_monitor;
pub use disk_monitor::DiskMonitor;

use anyhow::Result;
use chrono::Local;
use serde_json;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::config::Config;
use crate::memory::{MemoryChunk, MemoryManager};
use crate::scripting::ScriptService;
pub use client::{SmartClient, SmartResponse};
pub use compaction::{CompactionStrategy, NativeCompactor, ScriptCompactor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextStrategy {
    Full,
    Stateless,
    Episodic,
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: String,
    pub context_window: usize,
    pub reserve_tokens: usize,
}

#[derive(Clone)]
pub struct Agent {
    pub agent_id: String,
    config: AgentConfig,
    app_config: Config,
    memory: Arc<MemoryManager>,

    session_manager: SessionManager,
    tool_executor: ToolExecutor,
    memory_context: Arc<MemoryContextBuilder>,
    script_service: Option<ScriptService>,

    chat_engine: Arc<ChatEngine>,
    #[allow(dead_code)]
    // Kept for backward compatibility; may be used by external tools or future features. Not read in current code paths but part of Agent's composition.
    mcp_manager: Arc<McpManager>,
    disk_monitor: Arc<DiskMonitor>,

    /// Project working directory (Worksite)
    project_dir: PathBuf,
    status_lines: Vec<String>,

    cumulative_usage: Usage,
}

impl Agent {
    pub async fn new(
        config: AgentConfig,
        app_config: &Config,
        memory: MemoryManager,
        context_strategy: ContextStrategy,
        agent_id: &str,
    ) -> Result<Self> {
        Self::new_with_project(
            config,
            app_config,
            memory,
            context_strategy,
            std::env::current_dir()?,
            agent_id,
        )
        .await
    }

    pub async fn new_with_project(
        config: AgentConfig,
        app_config: &Config,
        memory: MemoryManager,
        _context_strategy: ContextStrategy,
        project_dir: PathBuf,
        agent_id: &str,
    ) -> Result<Self> {
        let client = SmartClient::new(app_config.clone(), config.model.clone());

        let memory = Arc::new(memory);
        let disk_monitor = DiskMonitor::new(app_config.disk.clone());

        let mut tools = tools::create_default_tools_with_project(
            app_config,
            Some(Arc::clone(&memory)),
            disk_monitor.clone(),
            project_dir.clone(),
        )?;

        // Initialize MCP Manager
        let (idle_timeout, health_check_interval) = if let Some(c) = &app_config.extensions.mcp {
            (c.idle_timeout_secs, c.health_check_interval_secs)
        } else {
            (600, 0)
        };
        let mcp_manager = McpManager::new_with_config(idle_timeout, health_check_interval);

        if let Some(mcp_config) = &app_config.extensions.mcp {
            if !mcp_config.servers.is_empty() {
                let server_configs: Vec<_> = mcp_config.servers.values().cloned().collect();
                mcp_manager.initialize(server_configs).await;

                // Connect to servers and load tools
                // We do this eagerly for now to register tools
                for (name, _) in &mcp_config.servers {
                    match mcp_manager.ensure_server(name).await {
                        Ok(_) => match mcp_manager.list_tools(name).await {
                            Ok(tool_defs) => {
                                for tool_def in tool_defs {
                                    if let (Some(tname), Some(desc), Some(schema)) = (
                                        tool_def.get("name").and_then(|v| v.as_str()),
                                        tool_def.get("description").and_then(|v| v.as_str()),
                                        tool_def.get("inputSchema"),
                                    ) {
                                        let tool = crate::agent::tools::mcp::McpTool::new(
                                            mcp_manager.clone(),
                                            name.clone(),
                                            tname.to_string(),
                                            desc.to_string(),
                                            schema.clone(),
                                        );
                                        tools.push(Arc::new(tool));
                                    }
                                }
                                info!("Loaded tools from MCP server: {}", name);
                            }
                            Err(e) => {
                                error!("Failed to list tools from MCP server {}: {}", name, e)
                            }
                        },
                        Err(e) => error!("Failed to connect to MCP server {}: {}", name, e),
                    }
                }
            }
        }

        // Load external tools from config
        for (name, conf) in &app_config.tools.external {
            let tool = crate::agent::tools::external::ExternalTool::new(
                name.clone(),
                conf.description.clone(),
                conf.command.clone(),
                conf.args.clone(),
                Some(project_dir.clone()),
                conf.sandbox,
            );
            tools.push(Arc::new(tool));
        }

        // Register system introspect tool
        // We need ScriptService to be passed in, or we can't fully initialize it here.
        // Agent is created BEFORE ScriptService usually?
        // Wait, main.rs creates ScriptService then Agent?
        // Actually, main.rs:
        // 1. Config::load()
        // 2. MemoryManager
        // 3. Agent::new_with_project -> creates tools
        // 4. ScriptService::new
        // So Agent doesn't have ScriptService yet.
        // We might need to inject it later or rework initialization order.
        // For now, let's skip ScriptService dependency or use a placeholder/Option.
        // But SystemIntrospectTool needs it.
        // Refactoring Agent creation is risky.
        // Let's defer SystemIntrospectTool registration to after ScriptService is available?
        // Agent::set_tools is available.
        // So in main.rs, after creating ScriptService, we can create SystemIntrospectTool and add it to Agent.
        // But Agent::new creates default tools.

        let session_manager = SessionManager::new(app_config.clone());
        let tool_executor = ToolExecutor::new(tools, app_config.clone());
        let memory_context = Arc::new(MemoryContextBuilder::new(
            memory.clone(),
            app_config.clone(),
        ));
        // disk_monitor moved up

        let chat_engine = ChatEngine::new(
            client,
            session_manager.clone(),
            tool_executor.clone(),
            app_config.clone(),
            config.clone(),
        );

        Ok(Self {
            agent_id: agent_id.to_string(),
            config,
            app_config: app_config.clone(),
            memory,
            session_manager,
            tool_executor,
            memory_context,
            script_service: None,
            chat_engine: Arc::new(chat_engine),
            mcp_manager,
            disk_monitor,
            project_dir,
            status_lines: Vec::new(),
            cumulative_usage: Usage::default(),
        })
    }

    pub fn set_script_service(&mut self, service: ScriptService) {
        self.script_service = Some(service);
    }

    pub fn set_status_lines(&mut self, status: Vec<String>) {
        self.status_lines = status;
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    /// Get the provider name for the current model (resolved from config)
    pub fn provider_name(&self) -> String {
        match self.chat_engine.client().resolve_config(&self.config.model) {
            Ok(cfg) => cfg.provider.unwrap_or_else(|| "unknown".to_string()),
            Err(_) => "unknown".to_string(),
        }
    }

    pub fn set_tools(&mut self, tools: Vec<Arc<dyn Tool>>) {
        self.tool_executor.set_tools(tools);
        self.update_chat_engine();

        if let Some(service) = &self.script_service {
            let tool_names: Vec<String> = self
                .tool_executor
                .tools()
                .iter()
                .map(|t| t.name().to_string())
                .collect();

            // We update parent context asynchronously/fire-and-forget logic (sort of)
            // But since set_parent_context is async, we need to spawn it or block?
            // Agent methods are synchronous (except async ones). set_tools is NOT async.
            // Wait, ScriptService::set_parent_context is async.
            // I should use block_on or spawn?
            // set_tools is synchronous here.
            // I can use `tokio::task::spawn`.
            let service = service.clone();
            let model = self.config.model.clone();
            let agent_id = self.agent_id.clone();
            tokio::spawn(async move {
                let _ = service
                    .set_parent_context(Some(model), Some(tool_names), None, Some(agent_id))
                    .await;
            });
        }
    }

    fn update_chat_engine(&mut self) {
        let client = SmartClient::new(self.app_config.clone(), self.config.model.clone());
        self.chat_engine = Arc::new(ChatEngine::new(
            client,
            self.session_manager.clone(),
            self.tool_executor.clone(),
            self.app_config.clone(),
            self.config.clone(),
        ));
    }

    pub fn requires_approval(&self, tool_name: &str) -> bool {
        self.tool_executor.requires_approval(tool_name)
    }

    pub fn approval_required_tools(&self) -> &[String] {
        self.tool_executor.approval_required_tools()
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.config.model = model.to_string();
        self.update_chat_engine();
        info!("Switched to model: {}", model);

        if let Some(service) = &self.script_service {
            let tool_names: Vec<String> = self
                .tool_executor
                .tools()
                .iter()
                .map(|t| t.name().to_string())
                .collect();
            let service = service.clone();
            let model = self.config.model.clone();
            let agent_id = self.agent_id.clone();
            tokio::spawn(async move {
                let _ = service
                    .set_parent_context(Some(model), Some(tool_names), None, Some(agent_id))
                    .await;
            });
        }

        Ok(())
    }

    pub async fn memory_chunk_count(&self) -> usize {
        self.memory.chunk_count().await.unwrap_or(0)
    }

    pub fn has_embeddings(&self) -> bool {
        self.memory.has_embeddings()
    }

    pub fn context_window(&self) -> usize {
        self.config.context_window
    }

    pub fn reserve_tokens(&self) -> usize {
        self.config.reserve_tokens
    }

    pub fn set_session(&mut self, session: Arc<RwLock<Session>>) {
        self.session_manager.set_session(session);
        self.update_chat_engine();
    }

    pub fn set_compaction_strategy(&mut self, strategy: Arc<dyn CompactionStrategy>) {
        self.session_manager.set_compaction_strategy(strategy);
        self.update_chat_engine();
    }

    pub fn set_context_strategy(&mut self, _strategy: ContextStrategy) {}

    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        self.tool_executor.tools()
    }

    pub async fn context_usage(&self) -> (usize, usize, usize) {
        let session_arc = self.session_manager.session();
        let used = session_arc.read().await.token_count();
        let available = self.config.context_window;
        let reserve = self.config.reserve_tokens;
        let usable = available.saturating_sub(reserve);
        (used, usable, available)
    }

    pub async fn export_markdown(&self) -> String {
        let session_arc = self.session_manager.session();
        let session = session_arc.read().await;
        let mut output = String::new();
        output.push_str("# Zier Alpha Session Export\n\n");
        output.push_str(&format!("Model: {}\n", self.config.model));
        output.push_str(&format!("Session ID: {}\n\n", session.id()));
        output.push_str("---\n\n");

        for msg in session.messages() {
            let role = match msg.role {
                Role::User => "**User**",
                Role::Assistant => "**Assistant**",
                Role::System => "**System**",
                Role::Tool => "**Tool**",
            };
            output.push_str(&format!("{}\n\n{}\n\n---\n\n", role, msg.content));
        }

        output
    }

    pub fn usage(&self) -> &Usage {
        &self.cumulative_usage
    }

    fn add_usage(&mut self, usage: Option<Usage>) {
        if let Some(u) = usage {
            self.cumulative_usage.input_tokens += u.input_tokens;
            self.cumulative_usage.output_tokens += u.output_tokens;
        }
    }

    pub async fn new_session(&mut self) -> Result<()> {
        let workspace_skills = skills::load_skills(self.memory.workspace()).unwrap_or_default();
        let skills_prompt = skills::build_skills_prompt(&workspace_skills);
        debug!("Loaded {} skills from workspace", workspace_skills.len());

        let tools = self.tool_executor.tools();
        let tool_names_str: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        let tool_names_slice: Vec<&str> = tool_names_str.iter().map(|s| s.as_str()).collect();

        // Build fallback system prompt using default builder
        let fallback_params =
            system_prompt::SystemPromptParams::new(self.memory.workspace(), &self.config.model)
                .with_project(&self.project_dir, self.app_config.workdir.clone())
                .with_tools(tool_names_slice)
                .with_skills_prompt(skills_prompt.clone())
                .with_status_lines(self.status_lines.clone());
        let fallback_prompt = system_prompt::build_system_prompt(fallback_params);

        // Determine if we should use a generator script
        let system_prompt = if let (Some(service), Some(script_path_str)) = (
            &self.script_service,
            &self.app_config.agent.system_prompt_script,
        ) {
            // Build context for generator
            let hostname = std::env::var("HOSTNAME")
                .or_else(|_| std::env::var("HOST"))
                .ok();
            let now = Local::now();
            let current_time = now.format("%Y-%m-%d %H:%M:%S").to_string();
            let timezone = now.format("%Z").to_string();
            let timezone = if timezone.is_empty() {
                None
            } else {
                Some(timezone)
            };
            let context = SystemPromptContext {
                workspace_dir: self.memory.workspace().to_string_lossy().into_owned(),
                project_dir: self.project_dir.to_str().map(|s| s.to_string()),
                model: self.config.model.clone(),
                tool_names: tool_names_str.clone(),
                hostname,
                current_time,
                timezone,
                skills_prompt: Some(skills_prompt),
                status_lines: if self.status_lines.is_empty() {
                    None
                } else {
                    Some(self.status_lines.clone())
                },
            };

            match serde_json::to_value(&context) {
                Ok(value) => {
                    match service
                        .evaluate_generator(Path::new(script_path_str), value)
                        .await
                    {
                        Ok(prompt) => prompt,
                        Err(e) => {
                            error!("System prompt generator failed: {}", e);
                            fallback_prompt
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to serialize generator context: {}", e);
                    fallback_prompt
                }
            }
        } else {
            fallback_prompt
        };

        let memory_context = self.memory_context.build_memory_context().await?;

        let full_context = if memory_context.is_empty() {
            system_prompt
        } else {
            format!(
                "{}\n\n---\n\n# Workspace Context\n\n{}",
                system_prompt, memory_context
            )
        };

        self.session_manager
            .new_session(&self.memory, None, || full_context)
            .await
    }

    pub async fn hydrate_from_file(&mut self, path: &PathBuf) -> Result<()> {
        self.session_manager.hydrate_from_file(path).await
    }

    pub async fn set_system_prompt(&mut self, prompt: &str) {
        self.session_manager
            .session()
            .write()
            .await
            .set_system_context(prompt.to_string());
    }

    pub async fn resume_session(&mut self, session_id: &str) -> Result<()> {
        self.session_manager.load_session(session_id).await
    }

    pub async fn chat(&mut self, message: &str) -> Result<String> {
        let (response, usage) = self.chat_engine.chat(message).await?;
        self.add_usage(usage);
        Ok(response)
    }

    pub async fn chat_with_images(
        &mut self,
        message: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<String> {
        let (response, usage) = self.chat_engine.chat_with_images(message, images).await?;
        self.add_usage(usage);
        Ok(response)
    }

    pub async fn execute_tool(&self, call: &ToolCall) -> Result<String> {
        self.tool_executor.execute_tool(call).await
    }

    pub async fn compact_session(&mut self) -> Result<(usize, usize)> {
        self.session_manager
            .compact_session(self.chat_engine.client())
            .await
    }

    pub async fn save_session_to_memory(&self) -> Result<Option<PathBuf>> {
        self.session_manager
            .save_session_to_memory(&self.memory)
            .await
    }

    pub async fn clear_session(&mut self) {
        self.session_manager.clear_session().await
    }

    pub async fn search_memory(&self, query: &str) -> Result<Vec<MemoryChunk>> {
        self.memory.search(query, 10).await
    }

    pub async fn reindex_memory(&self) -> Result<(usize, usize, usize)> {
        let stats = self.memory.reindex(true).await?;
        let (_, embedded) = self.memory.generate_embeddings(50).await?;
        Ok((stats.files_processed, stats.chunks_indexed, embedded))
    }

    pub async fn save_session(&self) -> Result<PathBuf> {
        self.session_manager.save_session().await
    }

    pub async fn save_session_for_agent(&self, agent_id: &str) -> Result<PathBuf> {
        self.session_manager.save_session_for_agent(agent_id).await
    }

    pub async fn session_status(&self) -> SessionStatus {
        self.session_manager
            .session_status(
                self.cumulative_usage.input_tokens,
                self.cumulative_usage.output_tokens,
            )
            .await
    }

    /// Get the current system prompt (context) of the active session.
    pub async fn system_prompt(&self) -> Option<String> {
        self.session_manager
            .session()
            .read()
            .await
            .system_context()
            .map(String::from)
    }

    pub async fn chat_stream(&mut self, message: &str) -> Result<StreamResult> {
        self.chat_engine.chat_stream(message).await
    }

    pub async fn chat_stream_with_images(
        &mut self,
        message: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<StreamResult> {
        self.chat_engine
            .chat_stream_with_images(message, images)
            .await
    }

    pub async fn finish_chat_stream(&mut self, response: &str) {
        self.chat_engine.finish_chat_stream(response).await
    }

    pub async fn execute_streaming_tool_calls(
        &mut self,
        _text_response: &str,
        _tool_calls: Vec<ToolCall>,
    ) -> Result<String> {
        anyhow::bail!("execute_streaming_tool_calls is deprecated")
    }

    pub fn provider(&self) -> SmartClient {
        self.chat_engine.client().clone()
    }

    pub async fn session_messages(&self) -> Vec<Message> {
        self.session_manager
            .session()
            .read()
            .await
            .messages_for_llm()
    }

    pub async fn raw_session_messages(&self) -> Vec<SessionMessage> {
        self.session_manager
            .session()
            .read()
            .await
            .raw_messages()
            .to_vec()
    }

    pub async fn add_user_message(&mut self, content: &str) {
        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::User,
                content: content.to_string(),
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });
    }

    pub async fn add_assistant_message(&mut self, content: &str) {
        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::Assistant,
                content: content.to_string(),
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });
    }

    pub async fn chat_stream_with_tools(
        &mut self,
        message: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>> + '_> {
        self.chat_engine
            .chat_stream_with_tools(message, images)
            .await
    }

    pub fn tool_schemas(&self) -> Vec<ToolSchema> {
        self.tool_executor.tool_schemas()
    }

    pub async fn provide_tool_result(&mut self, call_id: String, output: String) {
        self.chat_engine.provide_tool_result(call_id, output).await
    }

    pub fn approve_tool_call(&self, call_id: &str) {
        self.tool_executor.approve_tool_call(call_id);
    }

    pub async fn resume_chat_stream_with_tools(
        &mut self,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>> + '_> {
        self.chat_engine.resume_chat_stream_with_tools().await
    }

    pub async fn auto_save_session(&self) -> Result<()> {
        if self.disk_monitor.is_degraded() {
            tracing::warn!("Skipping auto-save due to low disk space (degraded mode)");
            return Ok(());
        }
        self.session_manager.auto_save_session().await
    }

    pub async fn continue_chat(&mut self) -> Result<String> {
        let (response, usage) = self.chat_engine.continue_chat().await?;
        self.add_usage(usage);
        Ok(response)
    }
}
