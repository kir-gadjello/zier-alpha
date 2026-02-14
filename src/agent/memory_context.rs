use anyhow::Result;
use std::sync::Arc;
use tracing::info;
use crate::memory::MemoryManager;
use crate::config::Config;
use crate::agent::sanitize;

pub struct MemoryContextBuilder {
    memory: Arc<MemoryManager>,
    config: Config,
}

impl MemoryContextBuilder {
    pub fn new(memory: Arc<MemoryManager>, config: Config) -> Self {
        Self { memory, config }
    }

    pub async fn build_memory_context(&self) -> Result<String> {
        let mut context = String::new();
        let use_delimiters = self.config.tools.use_content_delimiters;

        // Show welcome message on brand new workspace (first run)
        if self.memory.is_brand_new() {
            context.push_str(FIRST_RUN_WELCOME);
            context.push_str("\n\n---\n\n");
            info!("First run detected - showing welcome message");
        }

        // Load IDENTITY.md first (OpenClaw-compatible: agent identity context)
        if let Ok(identity_content) = self.memory.read_identity_file().await {
            if !identity_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "IDENTITY.md",
                        &identity_content,
                        sanitize::MemorySource::Identity,
                    ));
                } else {
                    context.push_str("# Identity (IDENTITY.md)\n\n");
                    context.push_str(&identity_content);
                }
                context.push_str("\n\n---\n\n");
            }
        }

        // Load USER.md (OpenClaw-compatible: user info)
        if let Ok(user_content) = self.memory.read_user_file().await {
            if !user_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "USER.md",
                        &user_content,
                        sanitize::MemorySource::User,
                    ));
                } else {
                    context.push_str("# User Info (USER.md)\n\n");
                    context.push_str(&user_content);
                }
                context.push_str("\n\n---\n\n");
            }
        }

        // Load SOUL.md (persona/tone) - this defines who the agent is
        if let Ok(soul_content) = self.memory.read_soul_file().await {
            if !soul_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "SOUL.md",
                        &soul_content,
                        sanitize::MemorySource::Soul,
                    ));
                } else {
                    context.push_str(&soul_content);
                }
                context.push_str("\n\n---\n\n");
            }
        }

        // Load AGENTS.md (OpenClaw-compatible: list of connected agents)
        if let Ok(agents_content) = self.memory.read_agents_file().await {
            if !agents_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "AGENTS.md",
                        &agents_content,
                        sanitize::MemorySource::Agents,
                    ));
                } else {
                    context.push_str("# Available Agents (AGENTS.md)\n\n");
                    context.push_str(&agents_content);
                }
                context.push_str("\n\n---\n\n");
            }
        }

        // Load TOOLS.md (OpenClaw-compatible: local tool notes)
        if let Ok(tools_content) = self.memory.read_tools_file().await {
            if !tools_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "TOOLS.md",
                        &tools_content,
                        sanitize::MemorySource::Tools,
                    ));
                } else {
                    context.push_str("# Tool Notes (TOOLS.md)\n\n");
                    context.push_str(&tools_content);
                }
                context.push_str("\n\n---\n\n");
            }
        }

        // Load MEMORY.md if it exists
        if let Ok(memory_content) = self.memory.read_memory_file().await {
            if !memory_content.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "MEMORY.md",
                        &memory_content,
                        sanitize::MemorySource::Memory,
                    ));
                } else {
                    context.push_str("# Long-term Memory (MEMORY.md)\n\n");
                    context.push_str(&memory_content);
                }
                context.push_str("\n\n");
            }
        }

        // Load today's and yesterday's daily logs
        if let Ok(recent_logs) = self.memory.read_recent_daily_logs(2).await {
            if !recent_logs.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "memory/*.md",
                        &recent_logs,
                        sanitize::MemorySource::DailyLog,
                    ));
                } else {
                    context.push_str("# Recent Daily Logs\n\n");
                    context.push_str(&recent_logs);
                }
                context.push_str("\n\n");
            }
        }

        // Load HEARTBEAT.md if it exists
        if let Ok(heartbeat) = self.memory.read_heartbeat_file().await {
            if !heartbeat.is_empty() {
                if use_delimiters {
                    context.push_str(&sanitize::wrap_memory_content(
                        "HEARTBEAT.md",
                        &heartbeat,
                        sanitize::MemorySource::Heartbeat,
                    ));
                } else {
                    context.push_str("# Pending Tasks (HEARTBEAT.md)\n\n");
                    context.push_str(&heartbeat);
                }
                context.push('\n');
            }
        }

        Ok(context)
    }
}

/// Welcome message shown on first run (brand new workspace)
const FIRST_RUN_WELCOME: &str = r#"# Welcome to Zier Alpha

This is your first session. I've set up a fresh workspace for you.

## Quick Start

1. **Just chat** - I'm ready to help with coding, writing, research, or anything else
2. **Your memory files** are in the workspace:
   - `MEMORY.md` - I'll remember important things here
   - `SOUL.md` - Customize my personality and behavior
   - `HEARTBEAT.md` - Tasks for autonomous mode

## Tell Me About Yourself

What's your name? What kind of projects do you work on? Any preferences for how I should communicate?

I'll save what I learn to MEMORY.md so I remember it next time."#;
