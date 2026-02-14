use anyhow::Result;
use clap::Parser;

mod cli;

use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Supervisor mode: intercept before runtime start
    if cli.supervised {
        return run_supervised();
    }

    // Handle daemon start/restart specially - must fork BEFORE starting Tokio runtime
    #[cfg(unix)]
    if let Commands::Daemon(ref args) = cli.command {
        match args.command {
            cli::daemon::DaemonCommands::Start { foreground: false } => {
                // Do the fork synchronously, then start Tokio in the child
                return cli::daemon::daemonize_and_run(&cli.agent);
            }
            cli::daemon::DaemonCommands::Restart { foreground: false } => {
                // Stop first (synchronously), then fork and start
                cli::daemon::stop_sync()?;
                return cli::daemon::daemonize_and_run(&cli.agent);
            }
            _ => {}
        }
    }

    // For all other commands, start the async runtime normally
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

fn run_supervised() -> Result<()> {
    use std::process::{Command, exit};
    use std::time::Duration;
    use std::thread::sleep;

    let args: Vec<String> = std::env::args()
        .filter(|a| a != "--supervised")
        .collect();

    if args.len() < 2 {
        // If run with just `zier-alpha --supervised`, default to daemon start foreground?
        // Or just fail if no subcommand.
        // But args[0] is binary path.
    }

    let bin = &args[0];
    let child_args = &args[1..];

    println!("Starting supervisor for: {} {:?}", bin, child_args);
    use std::io::Write;
    let _ = std::io::stdout().flush();

    let mut crash_count = 0;
    let mut last_start = std::time::Instant::now();

    loop {
        let mut child = Command::new(bin)
            .args(child_args)
            .spawn()
            .expect("Failed to spawn child process");

        let status = child.wait().expect("Failed to wait on child");

        if status.success() {
            println!("Child exited successfully.");
            exit(0);
        } else {
            let code = status.code().unwrap_or(-1);
            println!("Child exited with error code: {}", code);

            // Reset crash count if it ran for a while
            if last_start.elapsed() > Duration::from_secs(60) {
                crash_count = 0;
            } else {
                crash_count += 1;
            }
            last_start = std::time::Instant::now();

            let delay = std::cmp::min(crash_count * 2, 60);
            println!("Restarting in {} seconds...", delay);
            let _ = std::io::stdout().flush();
            sleep(Duration::from_secs(delay as u64));
        }
    }
}

async fn async_main(cli: Cli) -> Result<()> {
    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    match cli.command {
        Commands::Chat(args) => cli::chat::run(args, &cli.agent).await,
        Commands::Ask(args) => cli::ask::run(args, &cli.agent).await,
        #[cfg(feature = "desktop")]
        Commands::Desktop(args) => cli::desktop::run(args, &cli.agent),
        Commands::Daemon(args) => cli::daemon::run(args, &cli.agent).await,
        Commands::Memory(args) => cli::memory::run(args, &cli.agent).await,
        Commands::Config(args) => cli::config::run(args).await,
    }
}
