use anyhow::Result;
use clap::{Args, Subcommand};
use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use daemonize::Daemonize;

use zier_alpha::concurrency::TurnGate;
use zier_alpha::config::Config;
use zier_alpha::heartbeat::HeartbeatRunner;
use zier_alpha::ingress::IngressBus;
use zier_alpha::ingress::controller::ingress_loop;
use zier_alpha::memory::MemoryManager;
use zier_alpha::prompts::PromptRegistry;
use zier_alpha::scheduler::Scheduler;
use zier_alpha::server::{Server, telegram_polling::TelegramPollingService};
use zier_alpha::config::TelegramMode;
use zier_alpha::scripting::ScriptService;
use zier_alpha::scripting::loader::ScriptLoader;
use zier_alpha::agent::ScriptTool;

/// Synchronously stop the daemon (for use before Tokio runtime starts)
pub fn stop_sync() -> Result<()> {
    let pid_file = get_pid_file()?;

    if !pid_file.exists() {
        println!("Daemon is not running");
        return Ok(());
    }

    let pid = fs::read_to_string(&pid_file)?.trim().to_string();

    if !is_process_running(&pid) {
        println!("Daemon is not running (stale PID file)");
        fs::remove_file(&pid_file)?;
        return Ok(());
    }

    println!("Stopping daemon (PID: {})...", pid);

    // Send SIGTERM
    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("kill").args(["-TERM", &pid]).status()?;
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("taskkill").args(["/PID", &pid]).status()?;
    }

    // Wait for process to stop (up to 5 seconds)
    for _ in 0..50 {
        if !is_process_running(&pid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if is_process_running(&pid) {
        anyhow::bail!("Failed to stop daemon (PID: {})", pid);
    }

    println!("Daemon stopped");
    fs::remove_file(&pid_file).ok();

    Ok(())
}

/// Fork and daemonize BEFORE starting the Tokio runtime.
/// This avoids the macOS fork-safety issue with ObjC/Swift runtime.
#[cfg(unix)]
pub fn daemonize_and_run(agent_id: &str) -> Result<()> {
    let config = Config::load()?;

    // Check if already running
    let pid_file = get_pid_file()?;
    if pid_file.exists() {
        let pid = fs::read_to_string(&pid_file)?;
        if is_process_running(&pid) {
            anyhow::bail!("Daemon already running (PID: {})", pid.trim());
        }
        fs::remove_file(&pid_file)?;
    }

    let log_file = get_log_file(config.logging.retention_days)?;

    // Print startup info before daemonizing
    println!(
        "Starting Zier Alpha daemon in background (agent: {})...",
        agent_id
    );
    println!("  PID file: {}", pid_file.display());
    println!("  Log file: {}", log_file.display());
    if config.server.enabled {
        println!(
            "  Server: http://{}:{}",
            config.server.bind, config.server.port
        );
    }
    println!("\nUse 'zier-alpha daemon status' to check status");
    println!("Use 'zier-alpha daemon stop' to stop\n");

    // Fork BEFORE starting Tokio
    // Use append mode to preserve previous logs within the same day
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file)?;
    let stderr = stdout.try_clone()?;

    let daemonize = Daemonize::new()
        .pid_file(&pid_file)
        .working_directory(std::env::current_dir()?)
        .stdout(stdout)
        .stderr(stderr);

    match daemonize.start() {
        Ok(_) => {
            // Now in the child process - safe to start Tokio
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run_daemon_server(config, agent_id))
        }
        Err(e) => anyhow::bail!("Failed to daemonize: {}", e),
    }
}

/// Run the daemon server (called after fork in background mode)
async fn run_daemon_server(config: Config, agent_id: &str) -> Result<()> {
    // Initialize logging in the daemon process
    // Disable ANSI colors since we're writing to a file
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_ansi(false)
        .init();

    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), agent_id)?;
    let _watcher = memory.start_watcher()?;

    println!("Daemon started successfully");

    run_daemon_services(&config, agent_id).await?;

    println!("\nShutting down...");
    let pid_file = get_pid_file()?;
    fs::remove_file(&pid_file).ok();

    Ok(())
}

/// Run daemon services (server and/or heartbeat)
async fn run_daemon_services(config: &Config, agent_id: &str) -> Result<()> {
    // VIZIER: Initialize Ingress Bus
    let bus = std::sync::Arc::new(IngressBus::new(100));
    println!("  Ingress Bus: initialized");

    // VIZIER: Initialize Scripting Service
    // TODO: Load policy from config. For now, strict default.
    let script_service = ScriptService::new(Default::default())?;
    let script_loader = ScriptLoader::new(script_service.clone());

    if let Ok(home) = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))
    {
        let extensions_dir = home.home_dir().join(".zier-alpha/extensions");
        if extensions_dir.exists() {
            script_loader.load_from_dir(&extensions_dir).await?;
            println!("  Extensions: loaded from {}", extensions_dir.display());
        }
    }

    let script_tools_def = script_service.get_tools().await?;
    let mut script_tools: Vec<ScriptTool> = Vec::new();
    for def in script_tools_def {
        script_tools.push(ScriptTool::new(def, script_service.clone()));
    }
    println!("  Script Tools: {} loaded", script_tools.len());

    // VIZIER: Initialize Scheduler
    let mut scheduler = Scheduler::new(bus.clone()).await?;
    if let Ok(home) = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))
    {
        let scheduler_config_path = home.home_dir().join(".zier-alpha/scheduler.toml");
        if scheduler_config_path.exists() {
            scheduler.load_jobs(&scheduler_config_path).await?;
            println!(
                "  Scheduler: loaded from {}",
                scheduler_config_path.display()
            );
        }
    }
    scheduler.start().await?;
    println!("  Scheduler: started");

    // VIZIER: Initialize Prompt Registry
    let mut prompt_registry = PromptRegistry::new();
    if let Ok(home) = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))
    {
        let prompts_dir = home.home_dir().join(".zier-alpha/prompts");
        if prompts_dir.exists() {
            prompt_registry.load_from_dir(&prompts_dir)?;
            println!("  Prompts: loaded from {}", prompts_dir.display());
        }
    }
    let prompt_registry = std::sync::Arc::new(prompt_registry);

    // VIZIER: Start Ingress Consumer Loop
    let bus_receiver = bus.receiver();
    let config_clone = config.clone();
    let agent_id_clone = agent_id.to_string();
    let prompts_clone = prompt_registry.clone();
    let script_tools_clone = script_tools.clone();
    let jobs_clone = scheduler.jobs.clone();

    tokio::spawn(async move {
        ingress_loop(
            bus_receiver,
            config_clone,
            agent_id_clone,
            prompts_clone,
            script_tools_clone,
            jobs_clone,
        )
        .await;
    });
    println!("  Ingress Loop: started");

    // Start Telegram polling if enabled
    if config.server.enabled && config.server.telegram_mode == TelegramMode::Polling {
        if let Some(polling_service) = TelegramPollingService::new(config.clone(), bus.clone()) {
            tokio::spawn(async move {
                polling_service.run().await;
            });
            println!("  Telegram Polling: started");
        }
    }

    // Create shared turn gate for heartbeat + HTTP concurrency control
    let turn_gate = TurnGate::new();

    // Spawn heartbeat in background if enabled
    let heartbeat_handle = if config.heartbeat.enabled {
        let heartbeat_config = config.clone();
        let heartbeat_agent_id = agent_id.to_string();
        let heartbeat_gate = turn_gate.clone();
        println!(
            "  Heartbeat: enabled (interval: {})",
            config.heartbeat.interval
        );
        Some(tokio::spawn(async move {
            match HeartbeatRunner::new_with_gate(
                &heartbeat_config,
                &heartbeat_agent_id,
                Some(heartbeat_gate),
            ) {
                Ok(runner) => {
                    if let Err(e) = runner.run().await {
                        tracing::error!("Heartbeat runner error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to create heartbeat runner: {}", e);
                }
            }
        }))
    } else {
        None
    };

    // Run server or wait for shutdown
    if config.server.enabled {
        println!(
            "  Server: http://{}:{}",
            config.server.bind, config.server.port
        );
        let server = Server::new_with_gate(config, turn_gate, Some(bus.clone()))?;
        server.run().await?;
    } else if heartbeat_handle.is_some() {
        // Server not enabled but heartbeat is - wait for Ctrl+C
        println!("  Server: disabled");
        tokio::signal::ctrl_c().await?;
    } else {
        println!("  Neither server nor heartbeat is enabled. Use Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
    }

    // Abort heartbeat task on shutdown
    if let Some(handle) = heartbeat_handle {
        handle.abort();
    }

    Ok(())
}


#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub command: DaemonCommands,
}

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Start the daemon
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,
    },

    /// Stop the daemon
    Stop,

    /// Restart the daemon (stop then start)
    Restart {
        /// Run in foreground (don't daemonize)
        #[arg(short, long)]
        foreground: bool,
    },

    /// Show daemon status
    Status,

    /// Run heartbeat once (for testing)
    Heartbeat,
}

pub async fn run(args: DaemonArgs, agent_id: &str) -> Result<()> {
    match args.command {
        DaemonCommands::Start { foreground } => start_daemon(foreground, agent_id).await,
        DaemonCommands::Stop => stop_daemon().await,
        DaemonCommands::Restart { foreground } => restart_daemon(foreground, agent_id).await,
        DaemonCommands::Status => show_status().await,
        DaemonCommands::Heartbeat => run_heartbeat_once(agent_id).await,
    }
}

async fn start_daemon(foreground: bool, agent_id: &str) -> Result<()> {
    let config = Config::load()?;

    // Check if already running
    let pid_file = get_pid_file()?;
    if pid_file.exists() {
        let pid = fs::read_to_string(&pid_file)?;
        if is_process_running(&pid) {
            anyhow::bail!("Daemon already running (PID: {})", pid.trim());
        }
        fs::remove_file(&pid_file)?;
    }

    // Background mode on Unix is handled by daemonize_and_run() before Tokio starts
    // This function only handles foreground mode and non-Unix platforms
    #[cfg(unix)]
    if !foreground {
        // This shouldn't be reached - background mode is handled in main()
        anyhow::bail!("Background mode should be handled before Tokio starts");
    }

    #[cfg(not(unix))]
    if !foreground {
        println!(
            "Note: Background daemonization not supported on this platform. Running in foreground."
        );
    }

    println!(
        "Starting Zier Alpha daemon in foreground (agent: {})...",
        agent_id
    );

    // Write PID file for foreground mode
    fs::write(&pid_file, std::process::id().to_string())?;

    // Initialize components
    let memory = MemoryManager::new_with_full_config(&config.memory, Some(&config), agent_id)?;
    let _watcher = memory.start_watcher()?;

    println!("Daemon started successfully");

    run_daemon_services(&config, agent_id).await?;

    println!("\nShutting down...");
    fs::remove_file(&pid_file).ok();

    Ok(())
}

async fn stop_daemon() -> Result<()> {
    let pid_file = get_pid_file()?;

    if !pid_file.exists() {
        println!("Daemon is not running");
        return Ok(());
    }

    let pid = fs::read_to_string(&pid_file)?.trim().to_string();

    if !is_process_running(&pid) {
        println!("Daemon is not running (stale PID file)");
        fs::remove_file(&pid_file)?;
        return Ok(());
    }

    // Send SIGTERM
    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("kill").args(["-TERM", &pid]).status()?;
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("taskkill").args(["/PID", &pid]).status()?;
    }

    println!("Sent stop signal to daemon (PID: {})", pid);
    fs::remove_file(&pid_file)?;

    Ok(())
}

async fn restart_daemon(foreground: bool, agent_id: &str) -> Result<()> {
    // Stop the daemon if running
    let pid_file = get_pid_file()?;
    if pid_file.exists() {
        let pid = fs::read_to_string(&pid_file)?.trim().to_string();
        if is_process_running(&pid) {
            println!("Stopping daemon (PID: {})...", pid);

            #[cfg(unix)]
            {
                use std::process::Command;
                Command::new("kill").args(["-TERM", &pid]).status()?;
            }

            #[cfg(windows)]
            {
                use std::process::Command;
                Command::new("taskkill").args(["/PID", &pid]).status()?;
            }

            // Wait for process to stop (up to 5 seconds)
            for _ in 0..50 {
                if !is_process_running(&pid) {
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }

            if is_process_running(&pid) {
                anyhow::bail!("Failed to stop daemon (PID: {})", pid);
            }

            println!("Daemon stopped");
        }
        fs::remove_file(&pid_file).ok();
    }

    // For background mode on Unix, we need to exit and let main() handle daemonization
    #[cfg(unix)]
    if !foreground {
        println!("\nTo start daemon in background, run: zier-alpha daemon start");
        println!("(Background restart requires re-running the command due to fork requirements)");
        return Ok(());
    }

    // Start in foreground mode
    println!();
    start_daemon(foreground, agent_id).await
}

async fn show_status() -> Result<()> {
    let config = Config::load()?;
    let pid_file = get_pid_file()?;

    let running = if pid_file.exists() {
        let pid = fs::read_to_string(&pid_file)?;
        is_process_running(&pid)
    } else {
        false
    };

    println!("Zier Alpha Daemon Status");
    println!("----------------------");
    println!("Running: {}", if running { "yes" } else { "no" });

    if running {
        let pid = fs::read_to_string(&pid_file)?;
        println!("PID: {}", pid.trim());
    }

    println!("\nConfiguration:");
    println!("  Heartbeat enabled: {}", config.heartbeat.enabled);
    if config.heartbeat.enabled {
        println!("  Heartbeat interval: {}", config.heartbeat.interval);
    }
    println!("  Server enabled: {}", config.server.enabled);
    if config.server.enabled {
        println!(
            "  Server address: http://{}:{}",
            config.server.bind, config.server.port
        );
    }

    Ok(())
}

async fn run_heartbeat_once(agent_id: &str) -> Result<()> {
    let config = Config::load()?;
    let runner = HeartbeatRunner::new_with_agent(&config, agent_id)?;

    println!("Running heartbeat (agent: {})...", agent_id);
    let result = runner.run_once().await?;

    if result == "HEARTBEAT_OK" {
        println!("Heartbeat completed: No tasks needed attention");
    } else {
        println!("Heartbeat response:\n{}", result);
    }

    Ok(())
}

fn get_pid_file() -> Result<PathBuf> {
    // Put PID file in state dir (~/.zier-alpha/), not workspace
    let state_dir = zier_alpha::agent::get_state_dir()?;
    Ok(state_dir.join("daemon.pid"))
}

fn get_log_file(retention_days: u32) -> Result<PathBuf> {
    let state_dir = zier_alpha::agent::get_state_dir()?;
    let logs_dir = state_dir.join("logs");
    fs::create_dir_all(&logs_dir)?;

    // Prune old logs only if retention_days > 0
    if retention_days > 0 {
        prune_old_logs(&logs_dir, retention_days as i64);
    }

    // Use date-based log files (like OpenClaw)
    let date = chrono::Local::now().format("%Y-%m-%d");
    Ok(logs_dir.join(format!("zier-alpha-{}.log", date)))
}

/// Prune log files older than `keep_days` days
fn prune_old_logs(logs_dir: &std::path::Path, keep_days: i64) {
    let cutoff = chrono::Local::now() - chrono::Duration::days(keep_days);
    let cutoff_date = cutoff.format("%Y-%m-%d").to_string();

    if let Ok(entries) = fs::read_dir(logs_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Match zier-alpha-YYYY-MM-DD.log pattern
            if name_str.starts_with("zier-alpha-") && name_str.ends_with(".log") {
                if let Some(date_part) = name_str
                    .strip_prefix("zier-alpha-")
                    .and_then(|s| s.strip_suffix(".log"))
                {
                    if date_part < cutoff_date.as_str() {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

fn is_process_running(pid: &str) -> bool {
    let pid = pid.trim();

    #[cfg(unix)]
    {
        use std::process::Command;
        Command::new("kill")
            .args(["-0", pid])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(pid))
            .unwrap_or(false)
    }
}
