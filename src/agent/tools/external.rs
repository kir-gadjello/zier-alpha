use crate::agent::tools::Tool;
use crate::agent::providers::ToolSchema;
use crate::config::SandboxPolicy;
use crate::agent::tools::runner::run_sandboxed_tool;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use serde_json::Value;

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalTool {
    pub name: String,
    pub description: String,
    pub executable: String, // Path to interpreter or binary
    pub script_path: String, // Path to script (optional if executable is self-contained)
    pub arguments_schema: Value, // JSON schema for arguments
    pub sandbox: SandboxPolicy,
}

impl ExternalTool {
    #[allow(dead_code)]
    pub fn new(
        name: String,
        description: String,
        executable: String,
        script_path: String,
        arguments_schema: Value,
        sandbox: SandboxPolicy,
    ) -> Self {
        Self {
            name,
            description,
            executable,
            script_path,
            arguments_schema,
            sandbox,
        }
    }
}

#[async_trait]
impl Tool for ExternalTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.arguments_schema.clone(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        // Validate arguments against schema? (Optional, but good practice)
        // For now, we trust the LLM output is JSON and pass it to the runner.

        run_sandboxed_tool(
            &self.executable,
            &self.script_path,
            arguments,
            &self.sandbox
        ).await
    }
}
