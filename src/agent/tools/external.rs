use anyhow::{Result, Context};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Stdio;
use tokio::process::Command;
use crate::agent::tools::Tool;
use crate::agent::providers::ToolSchema;
use tracing::{debug, warn};

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

    #[cfg(target_os = "macos")]
    async fn run_sandboxed(&self, extra_args: &[String]) -> Result<String> {
        // macOS sandbox-exec
        // We use a strict profile denying network and limiting file access
        let profile = r#"
(version 1)
(allow default)
(deny network*)
(allow file-read* (subpath "/"))
(allow file-write* (subpath "/tmp"))
"#;
        // Note: Writing a temporary profile file is safer than passing inline if complex
        // For simplicity, we assume the command is simple.
        // But `sandbox-exec` expects the profile content via `-p`.

        let mut cmd = Command::new("sandbox-exec");
        cmd.arg("-p").arg(profile);
        cmd.arg(&self.command);
        cmd.args(&self.args);
        cmd.args(extra_args);

        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }

        self.run_command(cmd).await
    }

    #[cfg(target_os = "linux")]
    async fn run_sandboxed(&self, extra_args: &[String]) -> Result<String> {
        // Linux: use unshare to create new namespaces (net, ipc, uts, pid)
        // unshare -n -i -u -p -f --mount-proc cmd
        // Note: requires unprivileged user namespaces enabled

        let mut cmd = Command::new("unshare");
        cmd.args(&["-n", "-i", "-u", "-p", "-f", "--mount-proc"]);
        cmd.arg(&self.command);
        cmd.args(&self.args);
        cmd.args(extra_args);

        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }

        self.run_command(cmd).await
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    async fn run_sandboxed(&self, extra_args: &[String]) -> Result<String> {
        warn!("Sandboxing not supported on this platform, running normally");
        self.run_normal(extra_args).await
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
