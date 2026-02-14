use anyhow::Result;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use crate::agent::{Tool, ToolSchema, McpManager};
use crate::config::Config;
use crate::agent::disk_monitor::DiskMonitor;
use crate::scripting::ScriptService;

pub struct SystemIntrospectTool {
    config: Config,
    mcp_manager: Arc<McpManager>,
    script_service: ScriptService,
    disk_monitor: Arc<DiskMonitor>,
}

impl SystemIntrospectTool {
    pub fn new(
        config: Config,
        mcp_manager: Arc<McpManager>,
        script_service: ScriptService,
        disk_monitor: Arc<DiskMonitor>,
    ) -> Self {
        Self {
            config,
            mcp_manager,
            script_service,
            disk_monitor,
        }
    }
}

#[async_trait]
impl Tool for SystemIntrospectTool {
    fn name(&self) -> &str {
        "system_introspect"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "system_introspect".to_string(),
            description: "Query or control the Zier Alpha daemon runtime.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "enum": ["status", "mcp", "extensions", "restart_mcp", "reload_extension", "cleanup_disk"],
                        "description": "The command to execute"
                    },
                    "server": {
                        "type": "string",
                        "description": "Server name for restart_mcp"
                    },
                    "extension": {
                        "type": "string",
                        "description": "Extension name for reload_extension"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: serde_json::Value = serde_json::from_str(arguments)?;
        let command = args["command"].as_str().ok_or_else(|| anyhow::anyhow!("Missing command"))?;

        match command {
            "status" => {
                let status = json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "degraded_mode": self.disk_monitor.is_degraded(),
                    "server_enabled": self.config.server.enabled,
                    "heartbeat_enabled": self.config.heartbeat.enabled,
                });
                Ok(serde_json::to_string_pretty(&status)?)
            }
            "mcp" => {
                // We don't expose full MCP status yet, but we can list configured servers
                let servers: Vec<String> = self.config.extensions.mcp.as_ref()
                    .map(|c| c.servers.keys().cloned().collect())
                    .unwrap_or_default();
                Ok(serde_json::to_string_pretty(&servers)?)
            }
            "extensions" => {
                let list = self.script_service.list_extensions().await;
                Ok(serde_json::to_string_pretty(&list)?)
            }
            "restart_mcp" => {
                let server = args["server"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing server"))?;
                self.mcp_manager.shutdown(Some(server)).await;
                self.mcp_manager.ensure_server(server).await?;
                Ok(format!("Restarted MCP server: {}", server))
            }
            "reload_extension" => {
                let ext = args["extension"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing extension"))?;
                self.script_service.reload_extension(ext).await?;
                Ok(format!("Reloaded extension: {}", ext))
            }
            "cleanup_disk" => {
                // Trigger manual cleanup
                // We don't have this implemented in DiskMonitor yet, but we can pretend
                Ok("Disk cleanup triggered (not fully implemented)".to_string())
            }
            _ => Err(anyhow::anyhow!("Unknown command: {}", command))
        }
    }
}
