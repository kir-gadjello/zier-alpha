use anyhow::Result;
use clap::{Args, Subcommand};
use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use daemonize::Daemonize;

use localgpt::config::Config;
use localgpt::heartbeat::HeartbeatRunner;
use localgpt::memory::MemoryManager;
use localgpt::server::Server;

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

    /// Show daemon status
    Status,

    /// Run heartbeat once (for testing)
    Heartbeat,
}

pub async fn run(args: DaemonArgs, agent_id: &str) -> Result<()> {
    match args.command {
        DaemonCommands::Start { foreground } => start_daemon(foreground, agent_id).await,
        DaemonCommands::Stop => stop_daemon().await,
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
        // Check if process is still running
        if is_process_running(&pid) {
            anyhow::bail!("Daemon already running (PID: {})", pid.trim());
        }
        // Stale PID file, remove it
        fs::remove_file(&pid_file)?;
    }

    // Set up log file for daemon output
    let log_file = get_log_file()?;

    #[cfg(unix)]
    if !foreground {
        // Print startup info before daemonizing (user won't see output after fork)
        println!(
            "Starting LocalGPT daemon in background (agent: {})...",
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
        println!("\nUse 'localgpt daemon status' to check status");
        println!("Use 'localgpt daemon stop' to stop\n");

        // Proper Unix daemonization
        let stdout = std::fs::File::create(&log_file)?;
        let stderr = stdout.try_clone()?;

        let daemonize = Daemonize::new()
            .pid_file(&pid_file)
            .working_directory(std::env::current_dir()?)
            .stdout(stdout)
            .stderr(stderr);

        match daemonize.start() {
            Ok(_) => {
                // We are now running in the background
                // Continue with daemon setup below
            }
            Err(e) => anyhow::bail!("Failed to daemonize: {}", e),
        }
    } else {
        println!(
            "Starting LocalGPT daemon in foreground (agent: {})...",
            agent_id
        );
    }

    #[cfg(not(unix))]
    {
        if !foreground {
            println!(
                "Note: Background daemonization not supported on this platform. Running in foreground."
            );
        }
        println!("Starting LocalGPT daemon (agent: {})...", agent_id);
    }

    // Write PID file (only needed for foreground mode, daemonize handles it otherwise)
    #[cfg(unix)]
    if foreground {
        fs::write(&pid_file, std::process::id().to_string())?;
    }

    #[cfg(not(unix))]
    fs::write(&pid_file, std::process::id().to_string())?;

    // Initialize components
    let memory = MemoryManager::new_with_agent(&config.memory, agent_id)?;

    // Start memory file watcher
    let _watcher = memory.start_watcher()?;

    println!("Daemon started successfully");

    // Run heartbeat if enabled, otherwise just run the server
    // Note: We can't run both in parallel due to the SQLite/Send issue,
    // so we prioritize the HTTP server for now
    if config.server.enabled {
        println!(
            "  Server: http://{}:{}",
            config.server.bind, config.server.port
        );
        if config.heartbeat.enabled {
            println!(
                "  Heartbeat: running separately (use 'localgpt daemon heartbeat' to trigger)"
            );
        }

        let server = Server::new(&config)?;
        server.run().await?;
    } else if config.heartbeat.enabled {
        println!(
            "  Heartbeat: enabled (interval: {})",
            config.heartbeat.interval
        );
        let runner = HeartbeatRunner::new(&config)?;
        runner.run().await?;
    } else {
        println!("  Neither server nor heartbeat is enabled. Use Ctrl+C to stop.");
        // Just wait for shutdown signal
        tokio::signal::ctrl_c().await?;
    }

    println!("\nShutting down...");

    // Cleanup
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

async fn show_status() -> Result<()> {
    let config = Config::load()?;
    let pid_file = get_pid_file()?;

    let running = if pid_file.exists() {
        let pid = fs::read_to_string(&pid_file)?;
        is_process_running(&pid)
    } else {
        false
    };

    println!("LocalGPT Daemon Status");
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
            "  Server address: {}:{}",
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
    // Put PID file in state dir (~/.localgpt/), not workspace
    let state_dir = localgpt::agent::get_state_dir()?;
    Ok(state_dir.join("daemon.pid"))
}

fn get_log_file() -> Result<PathBuf> {
    let state_dir = localgpt::agent::get_state_dir()?;
    let logs_dir = state_dir.join("logs");
    fs::create_dir_all(&logs_dir)?;
    Ok(logs_dir.join("daemon.log"))
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
