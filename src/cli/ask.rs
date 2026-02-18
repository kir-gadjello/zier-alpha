use anyhow::Result;
use clap::Args;

use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy, ScriptTool};
use zier_alpha::concurrency::WorkspaceLock;
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;

use serde_json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[derive(Args)]
pub struct AskArgs {
    /// The question or task to perform
    pub question: String,

    /// Model to use (overrides config)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Output format: text (default) or json
    #[arg(short, long, default_value = "text")]
    pub format: String,

    /// Working directory for the project (Worksite)
    #[arg(short, long)]
    pub workdir: Option<String>,

    /// Output session result to JSON file (for Hive subagents)
    #[arg(long)]
    pub json_output: Option<String>,

    /// Hydrate session history from JSONL file
    #[arg(long)]
    pub hydrate_from: Option<String>,

    /// Run as a child process (skips workspace lock)
    #[arg(long)]
    pub child: bool,
}

pub async fn run(args: AskArgs, agent_id: &str) -> Result<()> {
    // Debug: print received question in child mode
    if args.child {
        tracing::debug!("[CHILD] received question: {}", args.question);
    }

    let config = Config::load()?;

    let project_dir = if let Some(w) = args.workdir {
        PathBuf::from(shellexpand::tilde(&w).to_string()).canonicalize()?
    } else {
        std::env::current_dir()?
    };

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), agent_id)?;

    let agent_config = AgentConfig {
        model: args.model.unwrap_or(config.agent.default_model.clone()),
        context_window: config.agent.context_window,
        reserve_tokens: config.agent.reserve_tokens,
    };

    // Capture model for parent context before moving agent_config
    let parent_model_for_context = agent_config.model.clone();

    let mut agent = Agent::new_with_project(
        agent_config,
        &config,
        memory,
        ContextStrategy::Stateless,
        project_dir.clone(),
        agent_id,
    )
    .await?;

    // Load extensions if enabled (BEFORE session creation)
    if let Some(ref hive_config) = config.extensions.hive {
        if hive_config.enabled {
            // Find hive extension path
            let mut hive_path = None;

            // 1. Check user config dir (~/.zier-alpha/extensions/hive/main.js)
            if let Ok(config_path) = Config::config_path() {
                if let Some(parent) = config_path.parent() {
                    let p = parent.join("extensions").join("hive").join("main.js");
                    if p.exists() {
                        hive_path = Some(p);
                    }
                }
            }

            // 2. Check relative to binary (dev/installed)
            if hive_path.is_none() {
                if let Ok(exe_path) = std::env::current_exe() {
                    if let Some(parent) = exe_path.parent() {
                        // Check ../extensions/hive/main.js (installed structure)
                        let p = parent.join("../extensions/hive/main.js");
                        if p.exists() {
                            hive_path = Some(p);
                        } else {
                            // Check extensions/hive/main.js (dev structure relative to target/debug or root)
                            // If running from cargo run, cwd is root, so try relative path
                            let p = std::env::current_dir()
                                .unwrap_or_default()
                                .join("extensions/hive/main.js");
                            if p.exists() {
                                hive_path = Some(p);
                            }
                        }
                    }
                }
            }

            if let Some(path) = hive_path {
                tracing::info!("Loading Hive extension from: {}", path.display());

                // Initialize ScriptService with extension policy
                let policy = crate::cli::common::make_extension_policy(
                    &project_dir,
                    &config.workspace_path(),
                );
                let service = ScriptService::new(
                    policy,
                    config.workspace_path(),
                    project_dir.clone(),
                    config.workdir.strategy.clone(),
                    None,
                    None,
                    Some(config.clone()),
                    agent_id.to_string(),
                );

                match service {
                    Ok(svc) => {
                        // Inject script service into agent for context propagation
                        agent.set_script_service(svc.clone());

                        if let Err(e) = svc.load_script(path.to_str().unwrap()).await {
                            tracing::error!("Failed to load Hive extension: {}", e);
                        } else {
                            // Register tools
                            match svc.get_tools().await {
                                Ok(tools) => {
                                    let mut current_tools = agent.tools().to_vec();
                                    for tool_def in tools {
                                        current_tools
                                            .push(Arc::new(ScriptTool::new(tool_def, svc.clone())));
                                    }
                                    agent.set_tools(current_tools);
                                    tracing::info!("Hive extension loaded successfully");

                                    // Propagate parent context to Hive for child agent inheritance
                                    let parent_model = parent_model_for_context.clone();
                                    let parent_tools: Vec<String> = agent
                                        .tools()
                                        .iter()
                                        .map(|t| t.name().to_string())
                                        .collect();
                                    tracing::info!("Hive parent_tools: {:?}", parent_tools);
                                    if let Err(e) = svc
                                        .set_parent_context(
                                            Some(parent_model),
                                            Some(parent_tools),
                                            None,
                                            Some(agent_id.to_string()),
                                        )
                                        .await
                                    {
                                        tracing::warn!(
                                            "Failed to set parent context on Hive: {}",
                                            e
                                        );
                                    }
                                }
                                Err(e) => tracing::error!(
                                    "Failed to get tools from Hive extension: {}",
                                    e
                                ),
                            }
                        }
                    }
                    Err(e) => tracing::error!("Failed to initialize ScriptService: {}", e),
                }
            } else {
                tracing::warn!("Hive extension enabled but main.js not found");
            }
        }
    }

    // Apply child tool restrictions if ZIER_CHILD_TOOLS is set (Hive inheritance)
    if let Ok(child_tools_json) = std::env::var("ZIER_CHILD_TOOLS") {
        eprintln!("[CHILD DEBUG] ZIER_CHILD_TOOLS: {}", child_tools_json);
        match serde_json::from_str::<Vec<String>>(&child_tools_json) {
            Ok(allowed_tools) => {
                eprintln!("[CHILD DEBUG] allowed_tools: {:?}", allowed_tools);
                let initial_tools: Vec<&str> = agent.tools().iter().map(|t| t.name()).collect();
                eprintln!("[CHILD DEBUG] initial tools: {:?}", initial_tools);
                let mut filtered = Vec::new();
                for tool in agent.tools() {
                    if allowed_tools.contains(&tool.name().to_string()) {
                        filtered.push(tool.clone());
                    }
                }
                let filtered_count = filtered.len();
                agent.set_tools(filtered);
                let final_tools: Vec<&str> = agent.tools().iter().map(|t| t.name()).collect();
                eprintln!("[CHILD DEBUG] final tools: {:?}", final_tools);
                tracing::info!(
                    "Child tool filtering applied: {} tools remaining",
                    filtered_count
                );
            }
            Err(e) => {
                eprintln!("[CHILD DEBUG] Invalid ZIER_CHILD_TOOLS JSON: {}", e);
                tracing::error!("Invalid ZIER_CHILD_TOOLS JSON: {}", e);
                // Fail hard to prevent security risks (unfiltered tools)
                anyhow::bail!("Invalid ZIER_CHILD_TOOLS JSON: {}", e);
            }
        }
    } else {
        eprintln!("[CHILD DEBUG] ZIER_CHILD_TOOLS not set");
    }

    agent.new_session().await?;

    if let Some(hydrate_path) = &args.hydrate_from {
        let path = PathBuf::from(hydrate_path);
        agent.hydrate_from_file(&path).await?;
        // Security cleanup: delete hydration file after reading
        tokio::fs::remove_file(&path).await.ok();
    }

    let workspace_lock = WorkspaceLock::new()?;
    let _lock_guard = if args.child {
        if std::env::var("ZIER_HIVE_DEPTH").is_err() {
            tracing::warn!("--child flag used but ZIER_HIVE_DEPTH not set");
        }
        tracing::info!("Running in child mode: assuming parent holds workspace lock");
        None
    } else {
        let lock_clone = workspace_lock.clone();
        Some(tokio::task::spawn_blocking(move || lock_clone.acquire()).await??)
    };

    // In child mode, capture timing and usage for metadata
    let start = Instant::now();
    let response = agent.chat(&args.question).await?;
    let latency_ms = start.elapsed().as_millis() as u64;
    let usage = agent.usage().clone();
    let model = agent.model().to_string();
    let provider = agent.provider_name();

    if let Some(json_path) = &args.json_output {
        let status = agent.session_status().await;
        let mut result = serde_json::json!({
            "version": "1.0",
            "session_id": status.id,
            "status": "success",
            "content": response,
            "artifacts": [], // TODO: capture artifacts
            "usage": usage,
        });

        // If running as a child (Hive), include metadata for parent
        if args.child {
            result["metadata"] = serde_json::json!({
                "agent": std::env::var("ZIER_HIVE_AGENT_NAME").ok().unwrap_or_else(|| "clone".to_string()),
                "model": model,
                "provider": provider,
                "latency_ms": latency_ms,
                "usage": {
                    "input_tokens": usage.input_tokens,
                    "output_tokens": usage.output_tokens,
                }
            });
        }

        let path = PathBuf::from(json_path);
        // Write to temp file first for atomicity
        let tmp_path = path.with_extension("tmp");
        {
            let content = serde_json::to_string_pretty(&result)?;
            tokio::fs::write(&tmp_path, content).await?;
        }
        tokio::fs::rename(tmp_path, path).await?;
    }

    match args.format.as_str() {
        "json" => {
            let output = serde_json::json!({
                "question": args.question,
                "response": response,
                "model": agent.model(),
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        _ => {
            println!("{}", response);
        }
    }

    Ok(())
}
