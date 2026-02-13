use anyhow::{Result, Context};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use crate::agent::tools::Tool;
use crate::agent::providers::ToolSchema;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct ExternalTool {
    name: String,
    description: String,
    command: String,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    sandbox: bool,
}

impl ExternalTool {
    pub fn new(
        name: String,
        description: String,
        command: String,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
        sandbox: bool
    ) -> Self {
        Self { name, description, command, args, working_dir, sandbox }
    }

    async fn run_sandboxed(&self, extra_args: &[String]) -> Result<String> {
        // Use shared runner for sandboxing
        // We need to construct full args list
        let mut full_args = self.args.clone();
        full_args.extend_from_slice(extra_args);

        let cwd = self.working_dir.clone().unwrap_or_else(|| std::path::PathBuf::from("."));

        debug!("Executing sandboxed external tool: {} {:?}", self.command, full_args);

        let output = crate::agent::tools::runner::run_sandboxed_command(&self.command, &full_args, &cwd, None).await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Ok(format!("Command failed ({}):\nSTDOUT: {}\nSTDERR: {}", output.status, stdout, stderr));
        }

        Ok(stdout.to_string())
    }

    async fn run_normal(&self, extra_args: &[String]) -> Result<String> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        cmd.args(extra_args);

        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }

        self.run_command(cmd).await
    }

    async fn run_command(&self, mut cmd: Command) -> Result<String> {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("Executing external tool {}: {:?}", self.name, cmd);

        let output = cmd.output().await.context("Failed to execute external command")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Ok(format!("Command failed ({}):\nSTDOUT: {}\nSTDERR: {}", output.status, stdout, stderr));
        }

        Ok(stdout.to_string())
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
            parameters: json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Additional arguments to append to the command"
                    }
                }
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args_val: Value = serde_json::from_str(arguments)?;
        let extra_args = args_val.get("args")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_default();

        if self.sandbox {
            self.run_sandboxed(&extra_args).await
        } else {
            self.run_normal(&extra_args).await
        }
    }
}
