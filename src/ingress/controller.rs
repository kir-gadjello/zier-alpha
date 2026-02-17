use crate::agent::tools::registry::ToolRegistry;
use crate::agent::DiskMonitor;
use crate::agent::{Agent, AgentConfig, ContextStrategy, ScriptTool};
use crate::config::Config;
use crate::ingress::{
    process_ingress_message, ApprovalCoordinator, DebounceManager, IngressMessage, TelegramClient,
};
use crate::memory::{ArtifactWriter, MemoryManager};
use crate::prompts::PromptRegistry;
use crate::scheduler::JobConfig;
use crate::scripting::ScriptService;
use crate::state::session_manager::GlobalSessionManager;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;
use tracing::{error, info, warn};

pub async fn ingress_loop(
    receiver: Arc<Mutex<mpsc::Receiver<IngressMessage>>>,
    config: Config,
    owner_agent_id: String,
    prompts: Arc<PromptRegistry>,
    script_service: ScriptService,
    jobs: Vec<JobConfig>,
    approval_coord: Arc<ApprovalCoordinator>,
) {
    let mut rx = receiver.lock().await;

    // Create MemoryManager: one owned for agent prototype, and a shared Arc for tasks
    let memory_obj =
        match MemoryManager::new_with_full_config(&config.memory, Some(&config), &owner_agent_id) {
            Ok(m) => m,
            Err(e) => {
                error!("Failed to initialize MemoryManager for ingress loop: {}", e);
                return;
            }
        };
    let memory = Arc::new(memory_obj.clone()); // shared across tasks

    // Artifact Writer
    let artifact_path = config.workspace_path().join("artifacts");
    let artifact_writer = Arc::new(ArtifactWriter::new(artifact_path));

    // Global Session Manager (wrap in Arc for sharing)
    let session_manager = Arc::new(GlobalSessionManager::new());

    // Telegram Client
    let telegram_client = config
        .server
        .telegram_bot_token
        .as_ref()
        .map(|t| Arc::new(TelegramClient::new(t.clone())));

    // Get initial script tools
    let script_tools_def = match script_service.get_tools().await {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to get tools: {}", e);
            Vec::new()
        }
    };
    let mut script_tools_vec: Vec<ScriptTool> = Vec::new();
    for def in script_tools_def {
        script_tools_vec.push(ScriptTool::new(def, script_service.clone()));
    }
    let script_tools = Arc::new(script_tools_vec);

    // Pre-build tools and agent prototype
    let project_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let disk_monitor = DiskMonitor::new(config.disk.clone());

    let base_tools = ToolRegistry::build(
        &config,
        Some(memory.clone()),
        disk_monitor.clone(),
        (*script_tools).clone(),
        project_dir.clone(),
    )
    .unwrap_or_else(|e| {
        error!("Failed to build initial tools: {}", e);
        Vec::new()
    });
    // base_tools is Vec<Arc<dyn Tool>>

    let agent_config = AgentConfig {
        model: config.agent.default_model.clone(),
        context_window: config.agent.context_window,
        reserve_tokens: config.agent.reserve_tokens,
    };

    let base_agent = match Agent::new_with_project(
        agent_config,
        &config,
        memory_obj, // pass owned MemoryManager
        ContextStrategy::Stateless,
        project_dir.clone(),
        &owner_agent_id,
    )
    .await
    {
        Ok(mut a) => {
            a.set_script_service(script_service.clone());
            a.set_tools(base_tools.clone());
            a
        }
        Err(e) => {
            error!("Failed to create base agent: {}", e);
            return;
        }
    };

    // Shared reference to jobs and config for cloning into tasks
    let jobs = Arc::new(jobs);
    let config = Arc::new(config.clone());

    // Debounce integration: spawn worker and forward messages
    let (debounce_tx, mut debounce_rx) = mpsc::unbounded_channel();
    let debounce_config = config.ingress.clone();

    // Move components into the debounce worker
    let session_manager = session_manager;
    let memory = memory;
    let base_agent = base_agent;
    let prompts = prompts;
    let artifact_writer = artifact_writer;
    let telegram_client = telegram_client;
    let jobs = jobs;
    let script_tools = script_tools;
    let config = config;
    let project_dir = project_dir;
    let base_tools = base_tools;
    let script_service = script_service;
    let disk_monitor = disk_monitor;
    let approval_coord = approval_coord;

    let worker = tokio::spawn(async move {
        let mut manager = DebounceManager::new(debounce_config);
        let mut ticker = tokio::time::interval(Duration::from_millis(500));

        loop {
            tokio::select! {
                msg_opt = debounce_rx.recv() => {
                    match msg_opt {
                        Some(msg) => manager.ingest(msg),
                        None => break,
                    }
                }
                _ = ticker.tick() => {
                    let now = Instant::now();
                    let ready = manager.flush_ready(now);
                    for msg in ready {
                        // Clone components for this processing task
                        let session_manager = session_manager.clone();
                        let memory = memory.clone();
                        let base_agent = base_agent.clone();
                        let prompts = prompts.clone();
                        let artifact_writer = artifact_writer.clone();
                        let telegram_client = telegram_client.clone();
                        let jobs = jobs.clone();
                        let script_tools = script_tools.clone();
                        let config = config.clone();
                        let project_dir = project_dir.clone();
                        let base_tools = base_tools.clone();
                        let script_service = script_service.clone();
                        let disk_monitor = disk_monitor.clone();
                        let approval_coord = approval_coord.clone();

                        tokio::spawn(async move {
                            process_ingress_message(
                                msg,
                                session_manager,
                                memory,
                                base_agent,
                                prompts,
                                artifact_writer,
                                telegram_client,
                                jobs,
                                script_tools,
                                config,
                                project_dir,
                                base_tools,
                                script_service,
                                disk_monitor,
                                approval_coord,
                            )
                            .await;
                        });
                    }
                }
            }
        }

        // Flush remaining messages after channel closed
        let final_msgs = manager.flush_all();
        for msg in final_msgs {
            let session_manager = session_manager.clone();
            let memory = memory.clone();
            let base_agent = base_agent.clone();
            let prompts = prompts.clone();
            let artifact_writer = artifact_writer.clone();
            let telegram_client = telegram_client.clone();
            let jobs = jobs.clone();
            let script_tools = script_tools.clone();
            let config = config.clone();
            let project_dir = project_dir.clone();
            let base_tools = base_tools.clone();
            let script_service = script_service.clone();
            let disk_monitor = disk_monitor.clone();
            let approval_coord = approval_coord.clone();

            tokio::spawn(async move {
                process_ingress_message(
                    msg,
                    session_manager,
                    memory,
                    base_agent,
                    prompts,
                    artifact_writer,
                    telegram_client,
                    jobs,
                    script_tools,
                    config,
                    project_dir,
                    base_tools,
                    script_service,
                    disk_monitor,
                    approval_coord,
                )
                .await;
            });
        }
    });

    // Forward incoming messages to the debounce worker
    while let Some(msg) = rx.recv().await {
        info!("Processing ingress: {}", msg);
        if debounce_tx.send(msg).is_err() {
            warn!("Debounce worker died, stopping ingress");
            break;
        }
    }
    drop(debounce_tx);
    let _ = worker.await;
}
