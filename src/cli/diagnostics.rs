use std::fs;
use std::path::Path;

use tracing::info;
use zier_alpha::config::Config;

/// Log bootup diagnostic info (to stdio and any tracing transports like Telegram).
pub async fn log_bootup(agent_id: &str, config: &Config, model: &str) {
    // Version from Cargo
    let version = env!("CARGO_PKG_VERSION");

    // Commit: try to read from .git/HEAD (best effort)
    let commit = get_git_commit();

    // Workspace path
    let workspace = config.workspace_path();

    // Enabled extensions
    let mut extensions: Vec<String> = Vec::new();
    if let Some(hc) = &config.extensions.hive {
        if hc.enabled {
            extensions.push("hive".to_string());
        }
    }
    if let Some(mcp) = &config.extensions.mcp {
        if !mcp.servers.is_empty() {
            extensions.push(format!("mcp({})", mcp.servers.len()));
        }
    }
    // Future extensions can be added here
    let extensions_str = extensions.join(", ");

    // MEMORY.md: size and approximate token count (1 token ≈ 4 chars)
    let mem_path = workspace.join("MEMORY.md");
    let (mem_size, mem_tokens) = match tokio::fs::metadata(&mem_path).await {
        Ok(meta) => {
            let size = meta.len();
            let tokens = (size as f64 / 4.0).round() as usize;
            (size, tokens)
        }
        Err(_) => (0, 0),
    };

    // Hostname (best effort)
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());

    // PID
    let pid = std::process::id();

    // Print in a concise, readable block
    info!(
        "🚀 Zier Alpha Bootup Diagnostic\n\
         Version: v{} (commit: {})\n\
         Model: {}\n\
         Workspace: {}\n\
         Extensions: {}\n\
         MEMORY.md: {} bytes (~{} tokens)\n\
         Agent ID: {}\n\
         Hostname: {}\n\
         PID: {}",
        version,
        commit.unwrap_or_else(|| "n/a".to_string()),
        model,
        workspace.display(),
        extensions_str,
        mem_size,
        mem_tokens,
        agent_id,
        hostname,
        pid
    );
}

fn get_git_commit() -> Option<String> {
    // Read .git/HEAD to get current commit or ref
    let head_path = Path::new(".git/HEAD");
    if !head_path.exists() {
        return None;
    }
    let head = fs::read_to_string(head_path).ok()?.trim().to_string();
    if head.starts_with("ref: ") {
        let ref_path = &head[5..];
        let commit_path = Path::new(".git").join(ref_path);
        if let Ok(commit) = fs::read_to_string(commit_path) {
            return Some(commit.trim().to_string());
        }
    } else if head.len() == 40 {
        // Detached HEAD with full SHA
        return Some(head);
    }
    None
}
