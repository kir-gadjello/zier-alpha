use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// Allow network access (system-socket, network*)
    #[serde(default)]
    pub allow_network: bool,

    /// List of paths/globs to allow read access to.
    /// Note: The executable itself and basic system libraries are always allowed.
    #[serde(default)]
    pub allow_read: Vec<String>,

    /// List of paths/globs to allow write access to.
    #[serde(default)]
    pub allow_write: Vec<String>,

    /// Allow reading environment variables
    #[serde(default)]
    pub allow_env: bool,

    /// Enforce OS-level sandboxing for external commands (unshare/sandbox-exec)
    #[serde(default)]
    pub enable_os_sandbox: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            allow_network: false,
            // By default, only allow reading from common locations if needed,
            // but the compiler will add the executable path.
            // A strict default might be empty.
            allow_read: vec![],
            // By default, allow writing to a specific temp directory if needed,
            // but for now we keep it strict.
            allow_write: vec![],
            allow_env: false,
            enable_os_sandbox: false,
        }
    }
}
