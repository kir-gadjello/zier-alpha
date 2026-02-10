use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use regex::Regex;
use once_cell::sync::Lazy;

#[derive(Debug, PartialEq, Eq)]
pub enum CommandSafety {
    Allowed,
    SoftBlock(String),
    RequireApproval(String),
    HardBlock(String),
}

pub struct SafetyPolicy {
    pub project_dir: PathBuf,
    pub workspace_dir: PathBuf,
    pub allow_shell_chaining: bool,
    pub allow_global_cwd: bool,
}

impl SafetyPolicy {
    pub fn new(project_dir: PathBuf, workspace_dir: PathBuf) -> Self {
        Self {
            project_dir,
            workspace_dir,
            allow_shell_chaining: false,
            allow_global_cwd: false,
        }
    }

    pub fn with_shell_chaining(mut self, allow: bool) -> Self {
        self.allow_shell_chaining = allow;
        self
    }

    pub fn with_global_cwd(mut self, allow: bool) -> Self {
        self.allow_global_cwd = allow;
        self
    }

    pub fn check_command(&self, cmd: &[String], cwd: Option<&Path>) -> Result<CommandSafety> {
        if cmd.is_empty() {
            return Err(anyhow!("Empty command"));
        }

        // 1. CWD Confinement
        if let Some(path) = cwd {
            if !self.allow_global_cwd {
                // Determine absolute path to check against
                let abs_cwd = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    self.project_dir.join(path)
                };

                // Attempt to canonicalize to resolve symlinks
                // Fallback to absolute path if file doesn't exist yet (though usually CWD must exist)
                let canonical_cwd = abs_cwd.canonicalize().unwrap_or(abs_cwd.clone());
                let canonical_project = self.project_dir.canonicalize().unwrap_or(self.project_dir.clone());
                let canonical_workspace = self.workspace_dir.canonicalize().unwrap_or(self.workspace_dir.clone());

                // Check if CWD is inside project_dir OR workspace_dir
                // Also allow /tmp and /var/tmp for temporary operations
                if !canonical_cwd.starts_with(&canonical_project)
                   && !canonical_cwd.starts_with(&canonical_workspace)
                   && !canonical_cwd.starts_with("/tmp")
                   && !canonical_cwd.starts_with("/var/tmp") {
                        return Ok(CommandSafety::HardBlock(format!(
                            "CWD confinement violation: {} is outside project/workspace",
                            path.display()
                        )));
                }
            }
        }

        // 2. Shell Chaining Detection
        if !self.allow_shell_chaining {
            let dangerous_chars = ["&&", "||", ";", "|", "`"];
            for arg in cmd {
                for char in dangerous_chars {
                    if arg.contains(char) {
                         return Ok(CommandSafety::HardBlock(format!(
                            "Shell chaining/injection detected ('{}') and allow_shell_chaining is false",
                            char
                        )));
                    }
                }
            }
        }

        let full_cmd_str = cmd.join(" ");

        // 3. Heuristic Blocking

        // HARD BLOCK
        static HARD_BLOCK_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"(rm\s+-rf\s+(/|~)|mkfs\.|dd\s+if=|:\(\)\{\s+:\|:&;?\};:)").unwrap());
        if HARD_BLOCK_REGEX.is_match(&full_cmd_str) {
             return Ok(CommandSafety::HardBlock("Destructive command detected (rm -rf root, mkfs, dd, fork bomb)".to_string()));
        }

        // REQUIRE APPROVAL
        static APPROVAL_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"(terraform\s+destroy|aws\s+.*\s+delete|az\s+group\s+delete|nmap|masscan)").unwrap());
        if APPROVAL_REGEX.is_match(&full_cmd_str) {
             return Ok(CommandSafety::RequireApproval(format!("Command requires approval: {}", full_cmd_str)));
        }

        // SOFT BLOCK
        if full_cmd_str.contains("grep -r /") {
             return Ok(CommandSafety::SoftBlock("Recursive grep on root detected. Please scope to project dir.".to_string()));
        }

        // 4. Tmux Payload Inspection
        // If the command is tmux, we inspect arguments for dangerous patterns
        if cmd[0] == "tmux" {
             if full_cmd_str.contains("cat /etc/shadow") || full_cmd_str.contains("sudo") {
                  return Ok(CommandSafety::HardBlock("Dangerous tmux payload detected".to_string()));
             }
        }

        Ok(CommandSafety::Allowed)
    }
}
