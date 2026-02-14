use crate::config::{SandboxPolicy, WorkdirStrategy};
use crate::scripting::deno::{DenoRuntime, DenoToolDefinition};
use anyhow::Result;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use std::thread;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::collections::HashMap;
use tracing::error;
use crate::ingress::IngressBus;
use crate::scheduler::Scheduler;
use crate::agent::mcp_manager::McpManager;

enum ScriptCommand {
    ExecuteTool {
        name: String,
        args: String,
        resp: oneshot::Sender<Result<String>>,
    },
    GetTools {
        resp: oneshot::Sender<Vec<DenoToolDefinition>>,
    },
    GetStatus {
        resp: oneshot::Sender<Result<Vec<String>>>,
    },
    Shutdown,
    SetParentContext {
        model: Option<String>,
        tools: Option<Vec<String>>,
        system_prompt_append: Option<String>,
    },
}

struct ExtensionHandle {
    sender: mpsc::Sender<ScriptCommand>,
    // We keep thread handle to join if needed, but for now we just let it run
    _thread: Option<thread::JoinHandle<()>>,
}

#[derive(Clone)]
pub struct ScriptService {
    // Map of extension_name -> Handle
    // For backward compatibility (loading script by path), we use path as key initially.
    // Ideally we should isolate per script.
    // To support `execute_tool` without knowing extension name, we need a lookup table for tools.
    extensions: Arc<RwLock<HashMap<String, ExtensionHandle>>>, // Key: script path
    tool_map: Arc<RwLock<HashMap<String, String>>>, // Tool Name -> Extension Path

    // Shared state for creating new runtimes
    policy: SandboxPolicy,
    workspace: PathBuf,
    project_dir: PathBuf,
    strategy: WorkdirStrategy,
    ingress_bus: Option<Arc<IngressBus>>,
    scheduler: Option<Arc<Mutex<Scheduler>>>,
    mcp_manager: Option<Arc<McpManager>>,

    // Parent context propagation (for Hive inheritance)
    parent_model: Arc<RwLock<Option<String>>>,
    parent_tools: Arc<RwLock<Option<Vec<String>>>>,
    parent_system_prompt_append: Arc<RwLock<Option<String>>>,
}

impl ScriptService {
    pub fn new(
        policy: SandboxPolicy,
        workspace: PathBuf,
        project_dir: PathBuf,
        strategy: WorkdirStrategy,
        ingress_bus: Option<Arc<IngressBus>>,
        scheduler: Option<Arc<Mutex<Scheduler>>>
    ) -> Result<Self> {
        let mcp_manager = Some(McpManager::new(600));

        Ok(Self {
            extensions: Arc::new(RwLock::new(HashMap::new())),
            tool_map: Arc::new(RwLock::new(HashMap::new())),
            policy,
            workspace,
            project_dir,
            strategy,
            ingress_bus,
            scheduler,
            mcp_manager,
            parent_model: Arc::new(RwLock::new(None)),
            parent_tools: Arc::new(RwLock::new(None)),
            parent_system_prompt_append: Arc::new(RwLock::new(None)),
        })
    }

    fn spawn_extension(&self, script_path: String) -> ExtensionHandle {
        let (tx, mut rx) = mpsc::channel(32);

        let policy = self.policy.clone();
        let workspace = self.workspace.clone();
        let project_dir = self.project_dir.clone();
        let strategy = self.strategy.clone();
        let ingress_bus = self.ingress_bus.clone();
        let scheduler = self.scheduler.clone();
        let mcp_manager = self.mcp_manager.clone();
        let script_path_clone = script_path.clone();

        let thread_handle = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();

            match runtime {
                Ok(rt) => {
                    rt.block_on(async move {
                        let mut deno = match DenoRuntime::new(policy, workspace, project_dir, strategy, ingress_bus, scheduler, mcp_manager) {
                            Ok(d) => d,
                            Err(e) => {
                                error!("Failed to initialize Deno runtime for {}: {}", script_path_clone, e);
                                return;
                            }
                        };

                        // Immediately load the script
                        if let Err(e) = deno.execute_script(&script_path_clone).await {
                             error!("Failed to load script {}: {}", script_path_clone, e);
                             // We continue running to handle potential get_tools/status calls which might return empty/error,
                             // but practically this extension is dead.
                        }

                        while let Some(cmd) = rx.recv().await {
                            match cmd {
                                ScriptCommand::ExecuteTool { name, args, resp } => {
                                    let res = deno.execute_tool(&name, &args).await;
                                    let _ = resp.send(res.map_err(|e| anyhow::anyhow!(e)));
                                }
                                ScriptCommand::GetTools { resp } => {
                                    let tools = deno.get_registered_tools();
                                    let _ = resp.send(tools);
                                }
                                ScriptCommand::GetStatus { resp } => {
                                    let res = deno.get_status().await;
                                    let _ = resp.send(res.map_err(|e| anyhow::anyhow!(e)));
                                }
                                ScriptCommand::Shutdown => break,
                                ScriptCommand::SetParentContext { model, tools, system_prompt_append } => {
                                    deno.set_parent_context(model, tools, system_prompt_append);
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to build runtime for extension {}: {}", script_path_clone, e);
                }
            }
        });

        ExtensionHandle {
            sender: tx,
            _thread: Some(thread_handle),
        }
    }

    pub async fn load_script(&self, path: &str) -> Result<()> {
        let path_string = path.to_string();

        // Spawn new isolate
        let handle = self.spawn_extension(path_string.clone());

        // Propagate parent context to this new extension BEFORE moving handle into map
        {
            let parent_model = self.parent_model.read().await.clone();
            let parent_tools = self.parent_tools.read().await.clone();
            let parent_spa = self.parent_system_prompt_append.read().await.clone();
            if parent_model.is_some() || parent_tools.is_some() || parent_spa.is_some() {
                let _ = handle.sender.send(ScriptCommand::SetParentContext { 
                    model: parent_model.clone(), 
                    tools: parent_tools.clone(), 
                    system_prompt_append: parent_spa 
                }).await;
            }
        }

        // Register handle
        {
            let mut exts = self.extensions.write().await;
            // If exists, shutdown old?
            if let Some(old) = exts.remove(&path_string) {
                let _ = old.sender.send(ScriptCommand::Shutdown).await;
            }
            exts.insert(path_string.clone(), handle);
        }

        // We don't need to send LoadScript command because spawn_extension loads it.
        // But we need to wait for it to load to get tools and register them in tool_map.
        let tools = {
            let exts = self.extensions.read().await;
            if let Some(handle) = exts.get(&path_string) {
                let (tx, rx) = oneshot::channel();
                // We assume the script loads synchronously-ish in the thread before processing commands.
                // But `execute_script` is async.
                // The loop starts AFTER execute_script returns or concurrently?
                // In `spawn_extension`, we do `deno.execute_script(...).await` BEFORE loop.
                // So if we send GetTools, it will process AFTER load finishes.
                handle.sender.send(ScriptCommand::GetTools { resp: tx }).await?;
                rx.await?
            } else {
                return Err(anyhow::anyhow!("Extension handle lost"));
            }
        };

        // Update tool map
        {
            let mut map = self.tool_map.write().await;
            for tool in tools {
                map.insert(tool.name, path_string.clone());
            }
        }

        Ok(())
    }

    pub async fn execute_tool(&self, name: &str, args: &str) -> Result<String> {
        let extension_path = {
            let map = self.tool_map.read().await;
            map.get(name).cloned()
        };

        if let Some(path) = extension_path {
            let exts = self.extensions.read().await;
            if let Some(handle) = exts.get(&path) {
                let (tx, rx) = oneshot::channel();
                handle.sender.send(ScriptCommand::ExecuteTool { name: name.to_string(), args: args.to_string(), resp: tx }).await?;
                return Ok(rx.await??);
            }
        }

        Err(anyhow::anyhow!("Tool not found or extension not loaded: {}", name))
    }

    pub async fn get_tools(&self) -> Result<Vec<DenoToolDefinition>> {
        let mut all_tools = Vec::new();
        let exts = self.extensions.read().await;

        for handle in exts.values() {
            let (tx, rx) = oneshot::channel();
            if handle.sender.send(ScriptCommand::GetTools { resp: tx }).await.is_ok() {
                if let Ok(tools) = rx.await {
                    all_tools.extend(tools);
                }
            }
        }
        Ok(all_tools)
    }

    pub async fn get_status_lines(&self) -> Result<Vec<String>> {
        let mut all_status = Vec::new();
        let exts = self.extensions.read().await;

        for handle in exts.values() {
            let (tx, rx) = oneshot::channel();
            if handle
                .sender
                .send(ScriptCommand::GetStatus { resp: tx })
                .await
                .is_ok()
            {
                if let Ok(Ok(lines)) = rx.await {
                    all_status.extend(lines);
                }
            }
        }
        Ok(all_status)
    }

    pub async fn list_extensions(&self) -> Vec<String> {
        let exts = self.extensions.read().await;
        exts.keys().cloned().collect()
    }

    pub async fn reload_extension(&self, name: &str) -> Result<()> {
        self.load_script(name).await
    }

    pub async fn set_parent_context(&self, model: Option<String>, tools: Option<Vec<String>>, system_prompt_append: Option<String>) -> Result<()> {
        // Update stored parent context
        *self.parent_model.write().await = model.clone();
        *self.parent_tools.write().await = tools.clone();
        *self.parent_system_prompt_append.write().await = system_prompt_append.clone();

        // Broadcast to all currently loaded extensions
        let exts = self.extensions.read().await;
        for handle in exts.values() {
            let _ = handle.sender.send(ScriptCommand::SetParentContext { 
                model: model.clone(), 
                tools: tools.clone(), 
                system_prompt_append: system_prompt_append.clone() 
            }).await;
        }
        Ok(())
    }
}