use anyhow::Result;
use clap::Args;

use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy};
use zier_alpha::concurrency::WorkspaceLock;
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;

use std::path::PathBuf;

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
}

pub async fn run(args: AskArgs, agent_id: &str) -> Result<()> {
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

    let mut agent = Agent::new_with_project(agent_config, &config, memory, ContextStrategy::Stateless, project_dir).await?;
    agent.new_session().await?;

    let workspace_lock = WorkspaceLock::new()?;
    let _lock_guard = workspace_lock.acquire()?;
    let response = agent.chat(&args.question).await?;

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
