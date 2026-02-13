use anyhow::Result;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tracing::warn;
use crate::agent::{Tool, ToolCall, ToolSchema};
use crate::config::Config;
use crate::agent::sanitize;

#[derive(Debug, thiserror::Error)]
#[error("Tool '{0}' requires approval")]
pub struct ApprovalRequiredError(pub String);

#[derive(Clone)]
pub struct ToolExecutor {
    tools: Vec<Arc<dyn Tool>>,
    config: Config,
    approval_manager: Arc<ApprovalManager>,
}

pub struct ApprovalManager {
    // Set of authorized tool call IDs
    approved_calls: Mutex<HashSet<String>>,
}

impl ApprovalManager {
    pub fn new() -> Self {
        Self {
            approved_calls: Mutex::new(HashSet::new()),
        }
    }

    pub fn approve(&self, call_id: &str) {
        let mut set = self.approved_calls.lock().unwrap();
        set.insert(call_id.to_string());
    }

    pub fn consume(&self, call_id: &str) -> bool {
        let mut set = self.approved_calls.lock().unwrap();
        set.remove(call_id)
    }
}

impl ToolExecutor {
    pub fn new(tools: Vec<Arc<dyn Tool>>, config: Config) -> Self {
        Self {
            tools,
            config,
            approval_manager: Arc::new(ApprovalManager::new()),
        }
    }

    pub fn tools(&self) -> &[Arc<dyn Tool>] {
        &self.tools
    }

    pub fn set_tools(&mut self, tools: Vec<Arc<dyn Tool>>) {
        self.tools = tools;
    }

    pub fn tool_schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    pub fn requires_approval(&self, tool_name: &str) -> bool {
        self.config
            .tools
            .require_approval
            .iter()
            .any(|t| t == tool_name)
    }

    pub fn approval_required_tools(&self) -> &[String] {
        &self.config.tools.require_approval
    }

    pub fn approve_tool_call(&self, call_id: &str) {
        self.approval_manager.approve(call_id);
    }

    pub async fn execute_tool(&self, call: &ToolCall) -> Result<String> {
        // Check approval
        if self.requires_approval(&call.name) {
            if !self.approval_manager.consume(&call.id) {
                // Return special error that ChatEngine can catch?
                // Or just bail.
                // ChatEngine catches ANY error and reports it.
                // But stream needs to know to emit ApprovalRequired event.
                // ChatEngine::stream_with_tool_loop handles this BEFORE calling execute_tool.
                // So if we are here, it means we are in non-streaming mode OR the engine failed to check.
                // If non-streaming, we fail hard.
                return Err(anyhow::Error::new(ApprovalRequiredError(call.name.clone())));
            }
        }

        for tool in &self.tools {
            if tool.name() == call.name {
                let raw_output = tool.execute(&call.arguments).await?;

                // Apply sanitization if configured
                if self.config.tools.use_content_delimiters {
                    let max_chars = if self.config.tools.tool_output_max_chars > 0 {
                        Some(self.config.tools.tool_output_max_chars)
                    } else {
                        None
                    };
                    let result = sanitize::wrap_tool_output(&call.name, &raw_output, max_chars);

                    // Log warnings for suspicious patterns
                    if self.config.tools.log_injection_warnings && !result.warnings.is_empty() {
                        warn!(
                            "Suspicious patterns detected in {} output: {:?}",
                            call.name,
                            result.warnings
                        );
                    }

                    return Ok(result.content);
                }

                return Ok(raw_output);
            }
        }
        anyhow::bail!("Unknown tool: {}", call.name)
    }
}
