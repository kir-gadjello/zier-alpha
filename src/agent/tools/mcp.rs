use crate::agent::mcp_manager::McpManager;
use crate::agent::providers::ToolSchema;
use crate::agent::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct McpTool {
    manager: Arc<McpManager>,
    server_name: String,
    name: String,
    description: String,
    schema: Value,
}

impl McpTool {
    pub fn new(
        manager: Arc<McpManager>,
        server_name: String,
        name: String,
        description: String,
        schema: Value,
    ) -> Self {
        Self {
            manager,
            server_name,
            name,
            description,
            schema,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.schema.clone(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;

        let params = json!({
            "name": self.name,
            "arguments": args
        });

        // Call tool on MCP server
        let result = self
            .manager
            .call(&self.server_name, "tools/call", params)
            .await?;

        // Parse result to string
        // MCP result usually contains `content` array
        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            let mut output = String::new();
            for item in content {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    output.push_str(text);
                }
            }
            if !output.is_empty() {
                return Ok(output);
            }
        }

        // Fallback: return raw JSON
        Ok(serde_json::to_string_pretty(&result)?)
    }
}
