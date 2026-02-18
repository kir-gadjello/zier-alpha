use crate::agent::providers::ToolSchema;
use crate::agent::tools::{resolve_path, Tool};
use crate::config::{SandboxPolicy, WorkdirStrategy};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct ExternalTool {
    name: String,
    description: String,
    command: String,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    sandbox: bool,
    policy: Option<SandboxPolicy>,
    path_args: Vec<String>,
    workspace: Option<PathBuf>,
    strategy: Option<WorkdirStrategy>,
}

impl ExternalTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        description: String,
        command: String,
        args: Vec<String>,
        working_dir: Option<PathBuf>,
        sandbox: bool,
        policy: Option<SandboxPolicy>,
        path_args: Vec<String>,
        workspace: Option<PathBuf>,
        strategy: Option<WorkdirStrategy>,
    ) -> Self {
        Self {
            name,
            description,
            command,
            args,
            working_dir,
            sandbox,
            policy,
            path_args,
            workspace,
            strategy,
        }
    }

    async fn run_sandboxed(&self, extra_args: &[String]) -> Result<String> {
        let policy = self
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Sandbox policy required for sandboxed execution"))?;

        let mut full_args = self.args.clone();
        full_args.extend_from_slice(extra_args);

        let cwd = self
            .working_dir
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("."));

        debug!(
            "Executing sandboxed external tool: {} {:?}",
            self.command, full_args
        );

        let output = crate::agent::tools::runner::run_sandboxed_command(
            &self.command,
            &full_args,
            &cwd,
            None,
            policy,
        )
        .await?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Ok(format!(
                "Command failed ({}):\nSTDOUT: {}\nSTDERR: {}",
                output.status, stdout, stderr
            ));
        }

        Ok(stdout.to_string())
    }

    async fn run_normal(&self, extra_args: &[String]) -> Result<String> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        cmd.args(extra_args);
        cmd.kill_on_drop(true);

        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }

        self.run_command(cmd).await
    }

    async fn run_command(&self, mut cmd: Command) -> Result<String> {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        debug!("Executing external tool {}: {:?}", self.name, cmd);

        let output = cmd
            .output()
            .await
            .context("Failed to execute external command")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Ok(format!(
                "Command failed ({}):\nSTDOUT: {}\nSTDERR: {}",
                output.status, stdout, stderr
            ));
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
        let mut properties = serde_json::Map::new();

        // Always include 'args' for backward compatibility and general usage
        properties.insert(
            "args".to_string(),
            json!({
                "type": "array",
                "items": { "type": "string" },
                "description": "Additional arguments to append to the command"
            }),
        );

        // Add path_args
        for arg_name in &self.path_args {
            properties.insert(
                arg_name.clone(),
                json!({
                    "type": "string",
                    "description": format!("Path argument: {}", arg_name)
                }),
            );
        }

        ToolSchema {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: Value::Object(serde_json::Map::from_iter(vec![
                ("type".to_string(), Value::String("object".to_string())),
                ("properties".to_string(), Value::Object(properties)),
            ])),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args_val: Value = serde_json::from_str(arguments)?;

        // 1. Collect standard 'args'
        let mut extra_args = args_val
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        // 2. Collect and resolve 'path_args'
        for arg_name in &self.path_args {
            if let Some(val) = args_val.get(arg_name) {
                if let Some(path_str) = val.as_str() {
                    // Resolve path
                    let resolved = if let (Some(ws), Some(strategy), Some(pd)) = (&self.workspace, &self.strategy, &self.working_dir) {
                        resolve_path(path_str, ws, pd, strategy)
                    } else {
                        // Fallback if context missing
                        PathBuf::from(path_str)
                    };

                    // Add resolved path to args.
                    // Note: We just append the path string.
                    // If the tool expects `--arg value`, the user should have configured `args` to include `--arg` or handle it.
                    // But typically `path_args` might be positional or implicit.
                    // To be safe and useful, we append it.
                    // A more advanced implementation would allow templating in `self.args`.
                    extra_args.push(resolved.to_string_lossy().to_string());
                }
            }
        }

        if self.sandbox {
            self.run_sandboxed(&extra_args).await
        } else {
            self.run_normal(&extra_args).await
        }
    }
}
