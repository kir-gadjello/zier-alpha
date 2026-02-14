use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::debug;

pub mod external;
pub mod runner;
pub mod script;
pub mod registry;
pub mod mcp;
pub mod system;

use super::providers::ToolSchema;
use crate::config::{Config, WorkdirStrategy};
use crate::memory::MemoryManager;
pub use script::ScriptTool;

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn schema(&self) -> ToolSchema;
    async fn execute(&self, arguments: &str) -> Result<String>;
}

pub fn create_default_tools(
    config: &Config,
    memory: Option<Arc<MemoryManager>>,
) -> Result<Vec<Arc<dyn Tool>>> {
    create_default_tools_with_project(config, memory, std::env::current_dir()?)
}

pub fn create_default_tools_with_project(
    config: &Config,
    memory: Option<Arc<MemoryManager>>,
    project_dir: PathBuf,
) -> Result<Vec<Arc<dyn Tool>>> {
    let workspace = config.workspace_path();

    // Use indexed memory search if MemoryManager is provided, otherwise fallback to grep-based
    let memory_search_tool: Arc<dyn Tool> = if let Some(ref mem) = memory {
        Arc::new(MemorySearchToolWithIndex::new(Arc::clone(mem)))
    } else {
        Arc::new(MemorySearchTool::new(workspace.clone()))
    };

    let strategy = &config.workdir.strategy;

    Ok(vec![
        Arc::new(BashTool::new(workspace.clone(), project_dir.clone(), strategy.clone(), config.tools.bash_timeout_ms)),
        Arc::new(ReadFileTool::new(workspace.clone(), project_dir.clone(), strategy.clone())),
        Arc::new(WriteFileTool::new(workspace.clone(), project_dir.clone(), strategy.clone())),
        Arc::new(EditFileTool::new(workspace.clone(), project_dir.clone(), strategy.clone())),
        memory_search_tool,
        Arc::new(MemoryGetTool::new(workspace)),
        Arc::new(WebFetchTool::new(config.tools.web_fetch_max_bytes)),
    ])
}

/// Helper to resolve a path based on strategy and cognitive routing
pub fn resolve_path(path: &str, workspace: &PathBuf, project_dir: &PathBuf, strategy: &WorkdirStrategy) -> PathBuf {
    let expanded = shellexpand::tilde(path).to_string();
    let p = PathBuf::from(&expanded);
    
    if p.is_absolute() {
        return p;
    }

    match strategy {
        WorkdirStrategy::Overlay => {
            if is_cognitive_path(&expanded) {
                workspace.join(p)
            } else {
                project_dir.join(p)
            }
        }
        WorkdirStrategy::Mount => {
            // In Mount mode, if path starts with "project/", we route to actual project_dir
            // Handle "project", "./project", "/project" (relative to virtual root)
            let clean_path = expanded.trim_start_matches("./").trim_start_matches('/');
            if clean_path == "project" || clean_path.starts_with("project/") {
                let rel = if clean_path == "project" {
                    PathBuf::new()
                } else {
                    PathBuf::from(&clean_path[8..]) // strip "project/"
                };
                project_dir.join(rel)
            } else {
                // Default to workspace
                workspace.join(p)
            }
        }
    }
}

/// Check if a path refers to a cognitive memory file
pub fn is_cognitive_path(path: &str) -> bool {
    let p = path.to_lowercase();
    p == "memory.md" || 
    p == "soul.md" || 
    p == "heartbeat.md" || 
    p == "identity.md" || 
    p == "user.md" || 
    p == "agents.md" ||
    p == "tools.md" ||
    p.starts_with("memory/")
}

// Bash Tool
pub struct BashTool {
    workspace: PathBuf,
    project_dir: PathBuf,
    strategy: WorkdirStrategy,
    default_timeout_ms: u64,
}

impl BashTool {
    pub fn new(workspace: PathBuf, project_dir: PathBuf, strategy: WorkdirStrategy, default_timeout_ms: u64) -> Self {
        Self { workspace, project_dir, strategy, default_timeout_ms }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".to_string(),
            description: "Execute a bash command and return the output".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": format!("Optional timeout in milliseconds (default: {})", self.default_timeout_ms)
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing command"))?;

        let timeout_ms = args["timeout_ms"]
            .as_u64()
            .unwrap_or(self.default_timeout_ms);

        let run_dir = match self.strategy {
            WorkdirStrategy::Overlay => &self.project_dir,
            WorkdirStrategy::Mount => &self.workspace,
        };

        debug!(
            "Executing bash command in {:?} (timeout: {}ms): {}",
            run_dir, timeout_ms, command
        );

        // Run command with timeout
        let timeout_duration = std::time::Duration::from_millis(timeout_ms);
        let output = tokio::time::timeout(
            timeout_duration,
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(command)
                .current_dir(run_dir)
                .output(),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Command timed out after {}ms", timeout_ms))??;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut result = String::new();

        if !stdout.is_empty() {
            result.push_str(&stdout);
        }

        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n\nSTDERR:\n");
            }
            result.push_str(&stderr);
        }

        if result.is_empty() {
            result = format!(
                "Command completed with exit code: {}",
                output.status.code().unwrap_or(-1)
            );
        }

        Ok(result)
    }
}

// Read File Tool
pub struct ReadFileTool {
    workspace: PathBuf,
    project_dir: PathBuf,
    strategy: WorkdirStrategy,
}

impl ReadFileTool {
    pub fn new(workspace: PathBuf, project_dir: PathBuf, strategy: WorkdirStrategy) -> Self {
        Self { workspace, project_dir, strategy }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read_file".to_string(),
            description: "Read the contents of a file".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to read (relative to workspace or absolute)"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (0-indexed)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path"))?;

        let resolved_path = resolve_path(path, &self.workspace, &self.project_dir, &self.strategy);

        debug!("Reading file: {}", resolved_path.display());

        let content = fs::read_to_string(&resolved_path)?;

        // Handle offset and limit
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().map(|l| l as usize);

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.min(total_lines);
        let end = limit
            .map(|l| (start + l).min(total_lines))
            .unwrap_or(total_lines);

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:4}\t{}", start + i + 1, line))
            .collect();

        Ok(selected.join("\n"))
    }
}

// Write File Tool
pub struct WriteFileTool {
    workspace: PathBuf,
    project_dir: PathBuf,
    strategy: WorkdirStrategy,
}

impl WriteFileTool {
    pub fn new(workspace: PathBuf, project_dir: PathBuf, strategy: WorkdirStrategy) -> Self {
        Self { workspace, project_dir, strategy }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write_file".to_string(),
            description: "Write content to a file (creates or overwrites)".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to write (relative to workspace or absolute)"
                    },
                    "content": {
                        "type": "string",
                        "description": "The content to write to the file"
                    }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing content"))?;

        let resolved_path = resolve_path(path, &self.workspace, &self.project_dir, &self.strategy);

        debug!("Writing file: {}", resolved_path.display());

        // Create parent directories if needed
        if let Some(parent) = resolved_path.parent() {
            fs::create_dir_all(parent)?;
        }

        fs::write(&resolved_path, content)?;

        Ok(format!(
            "Successfully wrote {} bytes to {}",
            content.len(),
            resolved_path.display()
        ))
    }
}

// Edit File Tool
pub struct EditFileTool {
    workspace: PathBuf,
    project_dir: PathBuf,
    strategy: WorkdirStrategy,
}

impl EditFileTool {
    pub fn new(workspace: PathBuf, project_dir: PathBuf, strategy: WorkdirStrategy) -> Self {
        Self { workspace, project_dir, strategy }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit_file".to_string(),
            description: "Edit a file by replacing old_string with new_string".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "The path to the file to edit (relative to workspace or absolute)"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "The text to replace"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "The replacement text"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences (default: false)"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path"))?;
        let old_string = args["old_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing old_string"))?;
        let new_string = args["new_string"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing new_string"))?;
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        let resolved_path = resolve_path(path, &self.workspace, &self.project_dir, &self.strategy);

        debug!("Editing file: {}", resolved_path.display());

        let content = fs::read_to_string(&resolved_path)?;

        let (new_content, count) = if replace_all {
            let count = content.matches(old_string).count();
            (content.replace(old_string, new_string), count)
        } else if content.contains(old_string) {
            (content.replacen(old_string, new_string, 1), 1)
        } else {
            return Err(anyhow::anyhow!("old_string not found in file"));
        };

        fs::write(&resolved_path, &new_content)?;

        Ok(format!("Replaced {} occurrence(s) in {}", count, resolved_path.display()))
    }
}

// Memory Search Tool
pub struct MemorySearchTool {
    workspace: PathBuf,
}

impl MemorySearchTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory_search".to_string(),
            description: "Search the memory index for relevant information".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing query"))?;
        let limit = args["limit"].as_u64().unwrap_or(5) as usize;

        debug!("Memory search: {} (limit: {})", query, limit);

        // Simple grep-based search for now
        // TODO: Use proper memory index
        let mut results = Vec::new();

        let memory_file = self.workspace.join("MEMORY.md");
        if memory_file.exists() {
            if let Ok(content) = fs::read_to_string(&memory_file) {
                for (i, line) in content.lines().enumerate() {
                    if line.to_lowercase().contains(&query.to_lowercase()) {
                        results.push(format!("MEMORY.md:{}: {}", i + 1, line));
                        if results.len() >= limit {
                            break;
                        }
                    }
                }
            }
        }

        // Search daily logs
        let memory_dir = self.workspace.join("memory");
        if memory_dir.exists() {
            if let Ok(entries) = fs::read_dir(&memory_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    if results.len() >= limit {
                        break;
                    }

                    let path = entry.path();
                    if path.extension().map(|e| e == "md").unwrap_or(false) {
                        if let Ok(content) = fs::read_to_string(&path) {
                            let filename = path.file_name().unwrap().to_string_lossy();
                            for (i, line) in content.lines().enumerate() {
                                if line.to_lowercase().contains(&query.to_lowercase()) {
                                    results.push(format!(
                                        "memory/{}:{}: {}",
                                        filename,
                                        i + 1,
                                        line
                                    ));
                                    if results.len() >= limit {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if results.is_empty() {
            Ok("No results found".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

// Memory Search Tool with Index - uses MemoryManager for hybrid FTS+vector search
pub struct MemorySearchToolWithIndex {
    memory: Arc<MemoryManager>,
}

impl MemorySearchToolWithIndex {
    pub fn new(memory: Arc<MemoryManager>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for MemorySearchToolWithIndex {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn schema(&self) -> ToolSchema {
        let description = if self.memory.has_embeddings() {
            "Search the memory index using hybrid semantic + keyword search for relevant information"
        } else {
            "Search the memory index for relevant information"
        };

        ToolSchema {
            name: "memory_search".to_string(),
            description: description.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing query"))?;
        let limit = args["limit"].as_u64().unwrap_or(5) as usize;

        let search_type = if self.memory.has_embeddings() {
            "hybrid"
        } else {
            "FTS"
        };
        debug!(
            "Memory search ({}): {} (limit: {})",
            search_type, query, limit
        );

        let results = self.memory.search(query, limit).await?;

        if results.is_empty() {
            return Ok("No results found".to_string());
        }

        // Format results with relevance scores
        let formatted: Vec<String> = results
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let preview: String = chunk.content.chars().take(200).collect();
                let preview = preview.replace('\n', " ");
                format!(
                    "{}. {} (lines {}-{}, score: {:.3})\n   {}{}",
                    i + 1,
                    chunk.file,
                    chunk.line_start,
                    chunk.line_end,
                    chunk.score,
                    preview,
                    if chunk.content.len() > 200 { "..." } else { "" }
                )
            })
            .collect();

        Ok(formatted.join("\n\n"))
    }
}

// Memory Get Tool - efficient snippet fetching after memory_search
pub struct MemoryGetTool {
    workspace: PathBuf,
}

impl MemoryGetTool {
    pub fn new(workspace: PathBuf) -> Self {
        Self { workspace }
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        // Handle paths relative to workspace
        if path.starts_with("memory/") || path == "MEMORY.md" || path == "HEARTBEAT.md" {
            self.workspace.join(path)
        } else {
            PathBuf::from(shellexpand::tilde(path).to_string())
        }
    }
}

#[async_trait]
impl Tool for MemoryGetTool {
    fn name(&self) -> &str {
        "memory_get"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "memory_get".to_string(),
            description: "Safe snippet read from MEMORY.md or memory/*.md with optional line range; use after memory_search to pull only the needed lines and keep context small.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file (e.g., 'MEMORY.md' or 'memory/2024-01-15.md')"
                    },
                    "from": {
                        "type": "integer",
                        "description": "Starting line number (1-indexed, default: 1)"
                    },
                    "lines": {
                        "type": "integer",
                        "description": "Number of lines to read (default: 50)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing path"))?;

        let from = args["from"].as_u64().unwrap_or(1).max(1) as usize;
        let lines_count = args["lines"].as_u64().unwrap_or(50) as usize;

        let resolved_path = self.resolve_path(path);

        debug!(
            "Memory get: {} (from: {}, lines: {})",
            resolved_path.display(),
            from,
            lines_count
        );

        if !resolved_path.exists() {
            return Ok(format!("File not found: {}", path));
        }

        let content = fs::read_to_string(&resolved_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Convert from 1-indexed to 0-indexed
        let start = (from - 1).min(total_lines);
        let end = (start + lines_count).min(total_lines);

        if start >= total_lines {
            return Ok(format!(
                "Line {} is past end of file ({} lines)",
                from, total_lines
            ));
        }

        let selected: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:4}\t{}", start + i + 1, line))
            .collect();

        let header = format!(
            "# {} (lines {}-{} of {})\n",
            path,
            start + 1,
            end,
            total_lines
        );
        Ok(header + &selected.join("\n"))
    }
}

// Web Fetch Tool
pub struct WebFetchTool {
    client: reqwest::Client,
    max_bytes: usize,
}

impl WebFetchTool {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            max_bytes,
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "web_fetch".to_string(),
            description: "Fetch content from a URL".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        let args: Value = serde_json::from_str(arguments)?;
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing url"))?;

        debug!("Fetching URL: {}", url);

        let response = self
            .client
            .get(url)
            .header("User-Agent", "Zier Alpha/0.1")
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        // Truncate if too long
        let truncated = if body.len() > self.max_bytes {
            format!(
                "{}...\n\n[Truncated, {} bytes total]",
                &body[..self.max_bytes],
                body.len()
            )
        } else {
            body
        };

        Ok(format!("Status: {}\n\n{}", status, truncated))
    }
}

/// Extract relevant detail from tool arguments for display.
/// Returns a human-readable summary of the key argument (file path, command, query, URL).
pub fn extract_tool_detail(tool_name: &str, arguments: &str) -> Option<String> {
    let args: Value = serde_json::from_str(arguments).ok()?;

    match tool_name {
        "edit_file" | "write_file" | "read_file" => args
            .get("path")
            .or_else(|| args.get("file_path"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        "bash" => args.get("command").and_then(|v| v.as_str()).map(|s| {
            if s.len() > 60 {
                format!("{}...", &s[..57])
            } else {
                s.to_string()
            }
        }),
        "memory_search" => args
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| format!("\"{}\"", s)),
        "web_fetch" => args
            .get("url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}
