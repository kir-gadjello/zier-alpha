use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, oneshot, mpsc};
use tokio::process::{Command, Child};
use std::process::Stdio;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use anyhow::{Result, anyhow, Context};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{info, error, debug};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default = "default_cache_dir")]
    pub cache_dir: String,

    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u64,

    #[serde(default = "default_strategy")]
    pub default_strategy: String,

    #[serde(default)]
    pub servers: HashMap<String, ServerConfig>,
}

fn default_cache_dir() -> String {
    if let Some(base) = directories::BaseDirs::new() {
        base.home_dir().join(".zier-alpha/cache/mcp").to_string_lossy().to_string()
    } else {
        "~/.zier-alpha/cache/mcp".to_string()
    }
}

fn default_idle_timeout() -> u64 {
    600
}

fn default_strategy() -> String {
    "hybrid".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ServerConfig {
    #[serde(default)]
    pub name: String, // Can be inferred from map key if missing
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub native_tools: Vec<String>,
}

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
    id: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponse {
    jsonrpc: String,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
    id: Option<u64>,
}

#[derive(Deserialize, Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    data: Option<serde_json::Value>,
}

struct ServerHandle {
    process: Arc<RwLock<Child>>,
    sender: mpsc::Sender<JsonRpcRequest>,
    last_used: Arc<RwLock<Instant>>,
    shutdown_tx: Arc<RwLock<Option<oneshot::Sender<()>>>>,
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<Result<serde_json::Value>>>>>,
}

pub struct McpManager {
    servers: Arc<RwLock<HashMap<String, ServerHandle>>>,
    configs: Arc<RwLock<HashMap<String, ServerConfig>>>,
    idle_timeout: Duration,
}

impl McpManager {
    pub fn new(idle_timeout_secs: u64) -> Arc<Self> {
        let manager = Arc::new(Self {
            servers: Arc::new(RwLock::new(HashMap::new())),
            configs: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
        });

        // Spawn background reaper
        let m = Arc::downgrade(&manager);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                if let Some(manager) = m.upgrade() {
                    manager.reap_idle_servers().await;
                } else {
                    break; // Manager dropped
                }
            }
        });

        manager
    }

    pub async fn initialize(&self, configs: Vec<ServerConfig>) {
        let mut map = self.configs.write().await;
        for config in configs {
            map.insert(config.name.clone(), config);
        }
    }

    // ... rest of methods ...
    pub async fn ensure_server(&self, server_name: &str) -> Result<()> {
        // 1. Optimistic check
        {
            let mut remove_stale = false;
            let servers = self.servers.read().await;
            if let Some(handle) = servers.get(server_name) {
                // Check if process is still running
                if let Ok(Some(_)) = handle.process.write().await.try_wait() {
                    remove_stale = true;
                } else {
                    let mut last_used = handle.last_used.write().await;
                    *last_used = Instant::now();
                    return Ok(());
                }
            }
            drop(servers); // Release read lock before acquiring write lock

            if remove_stale {
                info!("Removing stale MCP server: {}", server_name);
                let mut servers = self.servers.write().await;
                let is_dead = if let Some(handle) = servers.get(server_name) {
                    matches!(handle.process.write().await.try_wait(), Ok(Some(_)))
                } else {
                    false
                };
                if is_dead {
                    servers.remove(server_name);
                }
            }
        }

        // 2. Fetch config
        let config = {
            let configs = self.configs.read().await;
            configs.get(server_name).cloned().ok_or_else(|| anyhow!("Server config not found: {}", server_name))?
        };

        info!("Spawning MCP server: {} ({} {:?})", config.name, config.command, config.args);

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        if let Some(env) = config.env {
            cmd.envs(env);
        }

        let mut child = cmd.spawn().context("Failed to spawn MCP server process")?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("Failed to capture stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("Failed to capture stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("Failed to capture stderr"))?;

        let (tx, mut rx) = mpsc::channel::<JsonRpcRequest>(32);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let pending = Arc::new(RwLock::new(HashMap::<u64, oneshot::Sender<Result<serde_json::Value>>>::new()));

        // Writer task
        let mut writer_stdin = stdin;
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let json = serde_json::to_string(&req).unwrap();
                if let Err(e) = writer_stdin.write_all(json.as_bytes()).await {
                    error!("Failed to write to stdin: {}", e);
                    break;
                }
                if let Err(e) = writer_stdin.write_all(b"\n").await {
                    error!("Failed to write newline to stdin: {}", e);
                    break;
                }
                if let Err(e) = writer_stdin.flush().await {
                    error!("Failed to flush stdin: {}", e);
                    break;
                }
            }
        });

        // Reader task
        let reader_stdout = BufReader::new(stdout);
        let pending_clone = pending.clone();
        let server_name_clone = server_name.to_string();
        tokio::spawn(async move {
            let mut lines = reader_stdout.lines();
            loop {
                tokio::select! {
                    line = lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if line.trim().is_empty() { continue; }
                                match serde_json::from_str::<JsonRpcResponse>(&line) {
                                    Ok(resp) => {
                                        if let Some(id) = resp.id {
                                            let mut map = pending_clone.write().await;
                                            if let Some(tx) = map.remove(&id) {
                                                if let Some(error) = resp.error {
                                                    let _ = tx.send(Err(anyhow!("MCP Error {}: {}", error.code, error.message)));
                                                } else {
                                                    let _ = tx.send(Ok(resp.result.unwrap_or(serde_json::Value::Null)));
                                                }
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        debug!("[{}] Invalid JSON-RPC or notification: {} (Error: {})", server_name_clone, line, e);
                                    }
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                error!("[{}] Error reading stdout: {}", server_name_clone, e);
                                break;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        debug!("[{}] Shutdown signal received", server_name_clone);
                        break;
                    }
                }
            }
            // Notify any pending requests
            let mut map = pending_clone.write().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(anyhow!("Server terminated")));
            }
        });

        // Stderr logger
        let reader_stderr = BufReader::new(stderr);
        let server_name_clone_err = server_name.to_string();
        tokio::spawn(async move {
            let mut lines = reader_stderr.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                info!("[{}] stderr: {}", server_name_clone_err, line);
            }
        });

        // Handshake with cleanup on any error after this point
        let init_req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "zier-mcp-client", "version": "0.1.0" }
            }),
            id: Some(0),
        };

        let (resp_tx, resp_rx) = oneshot::channel();
        {
            let mut map = pending.write().await;
            map.insert(0, resp_tx);
        }

        // Send initialize request
        if let Err(e) = tx.send(init_req).await {
            // Cleanup
            let _ = shutdown_tx.send(());
            // Drop tx (implicitly when function returns)
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(anyhow!("Failed to send initialize request: {}", e));
        }

        // Wait for response with timeout
        match tokio::time::timeout(Duration::from_secs(10), resp_rx).await {
            Ok(Ok(Ok(_))) => {
                // Send 'notifications/initialized'
                let notif = JsonRpcRequest {
                    jsonrpc: "2.0".to_string(),
                    method: "notifications/initialized".to_string(),
                    params: serde_json::json!({}),
                    id: None,
                };
                if let Err(e) = tx.send(notif).await {
                    let _ = shutdown_tx.send(());
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Err(anyhow!("Failed to send initialized notification: {}", e));
                }
            }
            Ok(Ok(Err(e))) => {
                let _ = shutdown_tx.send(());
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(anyhow!("Initialize failed: {}", e));
            }
            Ok(Err(_)) => {
                let _ = shutdown_tx.send(());
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(anyhow!("Initialize response channel closed"));
            }
            Err(_) => {
                let _ = shutdown_tx.send(());
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(anyhow!("Initialize timed out"));
            }
        }

        // Create handle after successful handshake
        let handle = ServerHandle {
            process: Arc::new(RwLock::new(child)),
            sender: tx,
            last_used: Arc::new(RwLock::new(Instant::now())),
            shutdown_tx: Arc::new(RwLock::new(Some(shutdown_tx))),
            pending,
        };

        // Insert into map, handling concurrent insertion
        let mut servers = self.servers.write().await;
        if let Some(existing) = servers.get(server_name) {
            info!("Server {} started concurrently by another task. Using existing instance.", server_name);
            // Cleanup our redundant process
            if let Some(tx) = handle.shutdown_tx.write().await.take() {
                let _ = tx.send(());
            }
            let mut process = handle.process.write().await;
            let _ = process.kill().await;
            let _ = process.wait().await;
            // Update last_used on existing
            let mut last_used = existing.last_used.write().await;
            *last_used = Instant::now();
            return Ok(());
        }

        servers.insert(server_name.to_string(), handle);
        Ok(())
    }

    pub async fn list_tools(&self, server_name: &str) -> Result<Vec<serde_json::Value>> {
        let resp = self.call(server_name, "tools/list", serde_json::json!({})).await?;

        if let Some(tools) = resp.get("tools").and_then(|t| t.as_array()) {
            Ok(tools.clone())
        } else {
            Ok(Vec::new())
        }
    }

    pub async fn call(&self, server_name: &str, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let (sender, pending) = {
            let servers = self.servers.read().await;
            let handle = servers.get(server_name).ok_or_else(|| anyhow!("Server not connected: {}", server_name))?;

            // Update last used
            let mut last_used = handle.last_used.write().await;
            *last_used = Instant::now();

            (handle.sender.clone(), handle.pending.clone())
        };

        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
            id: Some(id),
        };

        let (tx, rx) = oneshot::channel();
        {
            let mut map = pending.write().await;
            map.insert(id, tx);
        }

        if let Err(_) = sender.send(req).await {
            let mut map = pending.write().await;
            map.remove(&id);
            return Err(anyhow!("Failed to send request (server likely dead)"));
        }

        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(Ok(res))) => Ok(res),
            Ok(Ok(Err(e))) => {
                 let mut map = pending.write().await;
                 map.remove(&id);
                 Err(e)
            }
            Ok(Err(_)) => {
                 let mut map = pending.write().await;
                 map.remove(&id);
                 Err(anyhow!("Response channel closed"))
            },
            Err(_) => {
                let mut map = pending.write().await;
                map.remove(&id);
                Err(anyhow!("Request timed out"))
            }
        }
    }

    pub async fn shutdown(&self, server_name: Option<&str>) {
        let mut servers = self.servers.write().await;
        if let Some(name) = server_name {
            if let Some(handle) = servers.remove(name) {
                let mut tx_lock = handle.shutdown_tx.write().await;
                if let Some(tx) = tx_lock.take() {
                    let _ = tx.send(());
                }
                let mut process = handle.process.write().await;
                let _ = process.kill().await;
                let _ = process.wait().await;
            }
        } else {
            for (_, handle) in servers.drain() {
                 let mut tx_lock = handle.shutdown_tx.write().await;
                 if let Some(tx) = tx_lock.take() {
                    let _ = tx.send(());
                 }
                 let mut process = handle.process.write().await;
                 let _ = process.kill().await;
                 let _ = process.wait().await;
            }
        }
    }

    async fn reap_idle_servers(&self) {
        let mut servers = self.servers.write().await;
        let now = Instant::now();
        let timeout = self.idle_timeout;

        let mut to_remove = Vec::new();

        for (name, handle) in servers.iter() {
            let last_used = *handle.last_used.read().await;
            if now.duration_since(last_used) > timeout {
                to_remove.push(name.clone());
            }
        }

        for name in to_remove {
            info!("Reaping idle MCP server: {}", name);
            if let Some(handle) = servers.remove(&name) {
                 let mut tx_lock = handle.shutdown_tx.write().await;
                 if let Some(tx) = tx_lock.take() {
                    let _ = tx.send(());
                 }
                 let mut process = handle.process.write().await;
                 let _ = process.kill().await;
                 let _ = process.wait().await;
            }
        }
    }
}
