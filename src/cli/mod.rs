pub mod ask;
pub mod chat;
pub mod common;
pub mod config;
pub mod daemon;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod memory;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "zier-alpha")]
#[command(author, version, about = "A lightweight, local-only AI assistant")]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Path to config file
    #[arg(short, long, global = true, env = "ZIER_ALPHA_CONFIG")]
    pub config: Option<String>,

    /// Agent ID to use (default: "main", OpenClaw-compatible)
    #[arg(
        short,
        long,
        global = true,
        default_value = "main",
        env = "ZIER_ALPHA_AGENT"
    )]
    pub agent: String,

    /// Run as supervisor (watches and restarts child process)
    #[arg(long, global = true)]
    pub supervised: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start an interactive chat session
    Chat(chat::ChatArgs),

    /// Ask a single question
    Ask(ask::AskArgs),

    /// Launch the desktop GUI
    #[cfg(feature = "desktop")]
    Desktop(desktop::DesktopArgs),

    /// Manage the daemon
    Daemon(daemon::DaemonArgs),

    /// Memory operations
    Memory(memory::MemoryArgs),

    /// Configuration management
    Config(config::ConfigArgs),
}
