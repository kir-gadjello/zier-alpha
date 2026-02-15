use anyhow::Result;
use clap::Args;
use futures::StreamExt;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::io::{self, Write};
use std::path::PathBuf;

use crate::cli::common::make_extension_policy;
use serde_json;
use std::sync::Arc;
use zier_alpha::agent::{
    extract_tool_detail, get_last_session_id_for_agent, get_skills_summary,
    list_sessions_for_agent, load_skills, parse_skill_command, search_sessions_for_agent, Agent,
    AgentConfig, ContextStrategy, ImageAttachment, ScriptTool, Skill,
};
use zier_alpha::concurrency::WorkspaceLock;
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;
use zier_alpha::scripting::ScriptService;

/// Adjust a byte index to the nearest valid UTF-8 char boundary (searching forward).
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Adjust a byte index to the nearest valid UTF-8 char boundary (searching forward).
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Extract a snippet from content, centered around the query match
fn extract_snippet(content: &str, query: &str, max_len: usize) -> String {
    // Normalize content: collapse whitespace and newlines
    let normalized: String = content.split_whitespace().collect::<Vec<_>>().join(" ");

    // Try to find query (case-insensitive)
    let lower_content = normalized.to_lowercase();
    let lower_query = query.to_lowercase();

    if let Some(pos) = lower_content.find(&lower_query) {
        // Center the snippet around the match
        let half_len = max_len / 2;
        let start = floor_char_boundary(&normalized, pos.saturating_sub(half_len));
        let end = ceil_char_boundary(
            &normalized,
            (pos + query.len() + half_len).min(normalized.len()),
        );

        // Adjust to word boundaries
        let mut snippet_start = start;
        let mut snippet_end = end;

        // Find word boundary at start
        if start > 0 {
            if let Some(space_pos) = normalized[start..].find(' ') {
                snippet_start = start + space_pos + 1;
            }
        }

        // Find word boundary at end
        if end < normalized.len() {
            if let Some(space_pos) = normalized[..end].rfind(' ') {
                snippet_end = space_pos;
            }
        }

        let snippet = &normalized[snippet_start..snippet_end];
        let prefix = if snippet_start > 0 { "..." } else { "" };
        let suffix = if snippet_end < normalized.len() {
            "..."
        } else {
            ""
        };

        format!("{}{}{}", prefix, snippet, suffix)
    } else {
        // No match found, show beginning of content
        let truncated: String = normalized.chars().take(max_len).collect();
        if normalized.len() > max_len {
            // Find last word boundary
            if let Some(space_pos) = truncated.rfind(' ') {
                format!("{}...", &truncated[..space_pos])
            } else {
                format!("{}...", truncated)
            }
        } else {
            truncated
        }
    }
}

#[derive(Args)]
pub struct ChatArgs {
    /// Model to use (overrides config)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Session ID to resume
    #[arg(short, long)]
    pub session: Option<String>,

    /// Resume the most recent session
    #[arg(long)]
    pub resume: bool,

    /// Show full tool arguments and outputs
    #[arg(short, long)]
    pub verbose: bool,

    /// Working directory for the project (Worksite)
    #[arg(short, long)]
    pub workdir: Option<String>,
}

pub async fn run(args: ChatArgs, agent_id: &str) -> Result<()> {
    let verbose_chat = args.verbose;
    let config = Config::load()?;

    let project_dir = if let Some(w) = args.workdir {
        PathBuf::from(shellexpand::tilde(&w).to_string()).canonicalize()?
    } else {
        std::env::current_dir()?
    };

    // Embedding provider is automatically created based on config.memory.embedding_provider
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
        ContextStrategy::Full,
        project_dir.clone(),
    )
    .await?;

    // Load enabled extensions (e.g., Hive) BEFORE session creation
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
            // 2. Check relative to binary/installed location
            if hive_path.is_none() {
                if let Ok(exe_path) = std::env::current_exe() {
                    if let Some(parent) = exe_path.parent() {
                        // Check ../extensions/hive/main.js (installed structure)
                        let p = parent.join("../extensions/hive/main.js");
                        if p.exists() {
                            hive_path = Some(p);
                        } else {
                            // Check extensions/hive/main.js relative to cwd (dev)
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
                let policy = make_extension_policy(&project_dir, &config.workspace_path());
                let service = match ScriptService::new(
                    policy,
                    config.workspace_path(),
                    project_dir.clone(),
                    config.workdir.strategy.clone(),
                    None,
                    None,
                    Some(config.clone()),
                ) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Failed to initialize ScriptService for Hive: {}", e);
                        return Err(e);
                    }
                };
                if let Err(e) = service.load_script(path.to_str().unwrap()).await {
                    tracing::error!("Failed to load Hive extension: {}", e);
                } else {
                    match service.get_tools().await {
                        Ok(tools) => {
                            let mut current_tools = agent.tools().to_vec();
                            for tool_def in tools {
                                current_tools
                                    .push(Arc::new(ScriptTool::new(tool_def, service.clone())));
                            }
                            agent.set_tools(current_tools);
                            tracing::info!("Hive extension loaded successfully");

                            // Propagate parent context to Hive for child agent inheritance
                            let parent_model = parent_model_for_context.clone();
                            let parent_tools: Vec<String> =
                                agent.tools().iter().map(|t| t.name().to_string()).collect();
                            if let Err(e) = service
                                .set_parent_context(Some(parent_model), Some(parent_tools), None)
                                .await
                            {
                                tracing::warn!("Failed to set parent context on Hive: {}", e);
                            }
                        }
                        Err(e) => tracing::error!("Failed to get tools from Hive extension: {}", e),
                    }
                }
            } else {
                tracing::warn!("Hive extension enabled but main.js not found");
            }
        }
    }

    // Apply child tool restrictions if ZIER_CHILD_TOOLS is set (Hive inheritance)
    if let Ok(child_tools_json) = std::env::var("ZIER_CHILD_TOOLS") {
        match serde_json::from_str::<Vec<String>>(&child_tools_json) {
            Ok(allowed_tools) => {
                let mut filtered = Vec::new();
                for tool in agent.tools() {
                    if allowed_tools.contains(&tool.name().to_string()) {
                        filtered.push(tool.clone());
                    }
                }
                let filtered_count = filtered.len();
                agent.set_tools(filtered);
                tracing::info!(
                    "Child tool filtering applied: {} tools remaining",
                    filtered_count
                );
            }
            Err(e) => {
                tracing::warn!("Invalid ZIER_CHILD_TOOLS JSON: {}", e);
            }
        }
    }

    let workspace_lock = WorkspaceLock::new()?;

    // Determine session to use
    let session_id = if let Some(id) = args.session {
        Some(id)
    } else if args.resume {
        get_last_session_id_for_agent(agent_id).await?
    } else {
        None
    };

    // Resume or create session (AFTER Hive loading)
    if let Some(session_id) = session_id {
        match agent.resume_session(&session_id).await {
            Ok(()) => {
                let status = agent.session_status().await;
                println!(
                    "Resumed session {} ({} messages)\n",
                    &session_id[..8],
                    status.message_count
                );
            }
            Err(e) => {
                eprintln!("Could not resume session: {}. Starting new session.\n", e);
                agent.new_session().await?;
            }
        }
    } else {
        agent.new_session().await?;
    }

    // Load skills from workspace
    let workspace = config.workspace_path();
    let skills = load_skills(&workspace).unwrap_or_default();
    let skills_count = skills.iter().filter(|s| s.eligibility.is_ready()).count();

    let embedding_status = if agent.has_embeddings() {
        " | Embeddings: enabled"
    } else {
        ""
    };
    let skills_status = if skills_count > 0 {
        format!(" | Skills: {}", skills_count)
    } else {
        String::new()
    };
    println!(
        "Zier Alpha v{} | Agent: {} | Model: {} | Memory: {} chunks{}{}\n",
        env!("CARGO_PKG_VERSION"),
        agent_id,
        agent.model(),
        agent.memory_chunk_count().await,
        embedding_status,
        skills_status
    );
    println!("Type /help for commands, /quit to exit\n");

    // Store agent_id for command handling
    let agent_id = agent_id.to_string();

    let mut rl = DefaultEditor::new()?;
    let mut stdout = io::stdout();

    // Track pending file attachments (text and images)
    enum Attachment {
        Text { name: String, content: String },
        Image { name: String, data: ImageAttachment },
    }
    let mut pending_attachments: Vec<Attachment> = Vec::new();

    loop {
        let readline = rl.readline("You: ");

        let input = match readline {
            Ok(line) => line,
            Err(ReadlineError::Interrupted) => {
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                break; // Ctrl+D
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        };

        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Add to history
        let _ = rl.add_history_entry(input);

        // Handle commands
        if input.starts_with('/') {
            // Special handling for /attach - adds to pending attachments
            if input.starts_with("/attach") {
                let parts: Vec<&str> = input.split_whitespace().collect();
                if parts.len() < 2 {
                    eprintln!("Usage: /attach <file_path>");
                    continue;
                }
                let file_path = parts[1..].join(" ");
                let expanded = shellexpand::tilde(&file_path).to_string();
                let path = std::path::Path::new(&expanded);
                let filename = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&file_path)
                    .to_string();

                // Check if it's an image file
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase());

                let is_image = matches!(
                    ext.as_deref(),
                    Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp")
                );

                if is_image {
                    // Read as binary and encode as base64
                    match tokio::fs::read(&expanded).await {
                        Ok(bytes) => {
                            use base64::{engine::general_purpose::STANDARD, Engine as _};
                            let data = STANDARD.encode(&bytes);
                            let media_type = match ext.as_deref() {
                                Some("png") => "image/png",
                                Some("jpg") | Some("jpeg") => "image/jpeg",
                                Some("gif") => "image/gif",
                                Some("webp") => "image/webp",
                                _ => "application/octet-stream",
                            }
                            .to_string();

                            let size = bytes.len();
                            pending_attachments.push(Attachment::Image {
                                name: filename.clone(),
                                data: ImageAttachment { data, media_type },
                            });
                            println!("Attached image: {} ({} bytes)", filename, size);
                            println!("Type your message to send with attachment(s), or /attachments to list.\n");
                        }
                        Err(e) => {
                            eprintln!("Failed to read image file: {}", e);
                        }
                    }
                } else {
                    // Read as text
                    match tokio::fs::read_to_string(&expanded).await {
                        Ok(content) => {
                            let size = content.len();
                            pending_attachments.push(Attachment::Text {
                                name: filename.clone(),
                                content,
                            });
                            println!("Attached: {} ({} bytes)", filename, size);
                            println!("Type your message to send with attachment(s), or /attachments to list.\n");
                        }
                        Err(e) => {
                            eprintln!("Failed to read file: {}", e);
                        }
                    }
                }
                continue;
            }

            // /attachments - list pending attachments
            if input == "/attachments" {
                if pending_attachments.is_empty() {
                    println!("\nNo pending attachments.\n");
                } else {
                    println!("\nPending attachments:");
                    for (i, att) in pending_attachments.iter().enumerate() {
                        match att {
                            Attachment::Text { name, content } => {
                                println!("  {}. [text] {} ({} bytes)", i + 1, name, content.len());
                            }
                            Attachment::Image { name, data } => {
                                println!(
                                    "  {}. [image] {} ({}, {} bytes encoded)",
                                    i + 1,
                                    name,
                                    data.media_type,
                                    data.data.len()
                                );
                            }
                        }
                    }
                    println!("\nType your message to send, or /clear-attachments to remove.\n");
                }
                continue;
            }

            // /clear-attachments - clear pending attachments
            if input == "/clear-attachments" {
                let count = pending_attachments.len();
                pending_attachments.clear();
                println!("\nCleared {} attachment(s).\n", count);
                continue;
            }

            match handle_command(input, &mut agent, &agent_id, &skills).await {
                CommandResult::Continue => continue,
                CommandResult::Quit => break,
                CommandResult::SendMessage(msg) => {
                    // Skill invocation - send message to agent
                    print!("\nZier Alpha: ");
                    stdout.flush().ok();
                    let lock_clone = workspace_lock.clone();
                    let _lock_guard =
                        tokio::task::spawn_blocking(move || lock_clone.acquire()).await??;
                    match agent.chat(&msg).await {
                        Ok(response) => {
                            println!("{}\n", response);
                            if let Err(e) = agent.auto_save_session().await {
                                eprintln!("Warning: Failed to auto-save session: {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error: {}\n", e);
                        }
                    }
                    continue;
                }
                CommandResult::Error(e) => {
                    eprintln!("Error: {}", e);
                    continue;
                }
            }
        }

        // Build message with attachments
        let mut message = input.to_string();
        let mut images: Vec<ImageAttachment> = Vec::new();

        if !pending_attachments.is_empty() {
            let mut text_attachments = Vec::new();

            // Separate text and image attachments
            for att in pending_attachments.drain(..) {
                match att {
                    Attachment::Text { name, content } => {
                        text_attachments.push((name, content));
                    }
                    Attachment::Image { data, .. } => {
                        images.push(data);
                    }
                }
            }

            // Add text attachments to message
            if !text_attachments.is_empty() {
                message.push_str("\n\n---\n\n**Attached files:**\n");
                for (name, content) in &text_attachments {
                    message.push_str(&format!("\n### {}\n```\n{}\n```\n", name, content));
                }
            }
        }

        // Send message to agent with streaming and tool support
        print!("\nZier Alpha: ");
        stdout.flush()?;

        let lock_clone = workspace_lock.clone();
        let _lock_guard = tokio::task::spawn_blocking(move || lock_clone.acquire()).await??;
        let mut current_stream: futures::stream::BoxStream<
            '_,
            Result<zier_alpha::agent::StreamEvent>,
        > = Box::pin(agent.chat_stream_with_tools(&message, images).await?);

        loop {
            let mut approval_info = None;

            {
                let mut pinned_stream = std::pin::pin!(current_stream);
                while let Some(event) = pinned_stream.next().await {
                    match event {
                        Ok(zier_alpha::agent::StreamEvent::Content(content)) => {
                            print!("{}", content);
                            stdout.flush()?;
                        }
                        Ok(zier_alpha::agent::StreamEvent::ToolCallStart {
                            name,
                            arguments,
                            ..
                        }) => {
                            let detail = extract_tool_detail(&name, &arguments);
                            if verbose_chat {
                                println!("\n[Tool Call: {}]", name);
                                println!("Arguments: {}", arguments);
                            } else if let Some(ref d) = detail {
                                println!("\n[{}: {}]", name, d);
                            } else {
                                println!("\n[{}]", name);
                            }
                            stdout.flush()?;
                        }
                        Ok(zier_alpha::agent::StreamEvent::ApprovalRequired {
                            name,
                            id,
                            arguments,
                        }) => {
                            let detail = extract_tool_detail(&name, &arguments);
                            if verbose_chat {
                                println!("\n[Approval Required: {}]", name);
                                println!("Arguments: {}", arguments);
                            } else if let Some(ref d) = detail {
                                println!("\n[{}: {}]", name, d);
                            } else {
                                println!("\n[{}]", name);
                            }

                            print!("Execute {}? [y/N]: ", name);
                            stdout.flush()?;

                            let mut input = String::new();
                            io::stdin().read_line(&mut input)?;
                            let input = input.trim().to_lowercase();

                            approval_info =
                                Some((id, name, arguments, input == "y" || input == "yes"));
                            break; // Exit inner stream loop
                        }
                        Ok(zier_alpha::agent::StreamEvent::ToolCallEnd { output, .. }) => {
                            if verbose_chat {
                                println!("Output: {}", output);
                            }
                            stdout.flush()?;
                        }
                        Ok(zier_alpha::agent::StreamEvent::Done) => {
                            // All done
                        }
                        Err(e) => {
                            eprintln!("\nStream error: {}", e);
                            break;
                        }
                    }
                }
            } // pinned_stream dropped here

            if let Some((id, name, arguments, approved)) = approval_info {
                let output = if approved {
                    let call = zier_alpha::agent::ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                    };
                    agent
                        .execute_tool(&call)
                        .await
                        .unwrap_or_else(|e| format!("Error: {}", e))
                } else {
                    println!("Skipped: {}", name);
                    "User denied execution".to_string()
                };

                agent.provide_tool_result(id, output).await;
                current_stream = Box::pin(agent.resume_chat_stream_with_tools().await?);
            } else {
                break;
            }
        }

        if let Err(e) = agent.auto_save_session().await {
            eprintln!("Warning: Failed to auto-save session: {}", e);
        }
        println!("\n");
    }

    println!("Goodbye!");
    Ok(())
}

enum CommandResult {
    Continue,
    Quit,
    SendMessage(String),
    Error(String),
}

async fn handle_command(
    input: &str,
    agent: &mut Agent,
    agent_id: &str,
    skills: &[Skill],
) -> CommandResult {
    let parts: Vec<&str> = input.split_whitespace().collect();
    let cmd = parts[0];

    match cmd {
        "/quit" | "/exit" | "/q" => CommandResult::Quit,

        "/help" | "/h" | "/?" => {
            println!("\nCommands:");
            println!("  /help, /h, /?     - Show this help");
            println!("  /quit, /exit, /q  - Exit chat");
            println!("  /new              - Start a fresh session (reloads memory context)");
            println!("  /skills           - List available skills");
            println!("  /sessions         - List available sessions");
            println!("  /search <query>   - Search across all sessions");
            println!("  /resume <id>      - Resume a specific session");
            println!("  /model [name]     - Show or switch model (e.g., /model gpt-4o)");
            println!("  /models           - List available model prefixes");
            println!("  /context          - Show context window usage");
            println!("  /export [file]    - Export session as markdown");
            println!("  /attach <file>    - Attach file to next message");
            println!("  /attachments      - List pending attachments");
            println!("  /compact          - Compact session history");
            println!("  /clear            - Clear session history (keeps context)");
            println!("  /memory <query>   - Search memory");
            println!("  /reindex          - Rebuild memory index");
            println!("  /save             - Save current session");
            println!("  /status           - Show session status and API token usage");

            // Show skill commands if any
            let invocable: Vec<&Skill> = skills.iter().filter(|s| s.can_invoke()).collect();
            if !invocable.is_empty() {
                println!("\nSkill commands:");
                for skill in invocable.iter().take(5) {
                    let emoji = skill
                        .emoji
                        .as_ref()
                        .map(|e| format!(" {}", e))
                        .unwrap_or_default();
                    println!("  /{}{} - {}", skill.command_name, emoji, skill.description);
                }
                if invocable.len() > 5 {
                    println!("  ... use /skills to see all");
                }
            }
            println!();
            CommandResult::Continue
        }

        "/skills" => {
            println!("\n{}\n", get_skills_summary(skills));
            CommandResult::Continue
        }

        "/sessions" => match list_sessions_for_agent(agent_id).await {
            Ok(sessions) => {
                if sessions.is_empty() {
                    println!("\nNo saved sessions found.\n");
                } else {
                    println!("\nAvailable sessions:");
                    for (i, session) in sessions.iter().take(10).enumerate() {
                        println!(
                            "  {}. {} ({} messages, {})",
                            i + 1,
                            &session.id[..8],
                            session.message_count,
                            session.created_at.format("%Y-%m-%d %H:%M")
                        );
                    }
                    if sessions.len() > 10 {
                        println!("  ... and {} more", sessions.len() - 10);
                    }
                    println!("\nUse /resume <id> to resume a session.\n");
                }
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to list sessions: {}", e)),
        },

        "/search" => {
            if parts.len() < 2 {
                return CommandResult::Error("Usage: /search <query>".into());
            }
            let query = parts[1..].join(" ");

            match search_sessions_for_agent(agent_id, &query).await {
                Ok(results) => {
                    if results.is_empty() {
                        println!("\nNo sessions found matching '{}'.\n", query);
                    } else {
                        println!("\nSessions matching '{}':", query);
                        for (i, result) in results.iter().take(10).enumerate() {
                            println!(
                                "  {}. {} ({} matches, {})",
                                i + 1,
                                &result.session_id[..8.min(result.session_id.len())],
                                result.match_count,
                                result.created_at.format("%Y-%m-%d")
                            );
                            if !result.message_preview.is_empty() {
                                println!("     \"{}\"", result.message_preview);
                            }
                        }
                        if results.len() > 10 {
                            println!("  ... and {} more", results.len() - 10);
                        }
                        println!("\nUse /resume <id> to resume a session.\n");
                    }
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Search failed: {}", e)),
            }
        }

        "/resume" => {
            if parts.len() < 2 {
                return CommandResult::Error("Usage: /resume <session-id>".into());
            }
            let session_id = parts[1];

            // Find session by prefix match
            match list_sessions_for_agent(agent_id).await {
                Ok(sessions) => {
                    let matching: Vec<_> = sessions
                        .iter()
                        .filter(|s| s.id.starts_with(session_id))
                        .collect();

                    match matching.len() {
                        0 => CommandResult::Error(format!(
                            "No session found matching '{}'",
                            session_id
                        )),
                        1 => {
                            let full_id = matching[0].id.clone();
                            match agent.resume_session(&full_id).await {
                                Ok(()) => {
                                    let status = agent.session_status().await;
                                    println!(
                                        "\nResumed session {} ({} messages)\n",
                                        &full_id[..8],
                                        status.message_count
                                    );
                                    CommandResult::Continue
                                }
                                Err(e) => CommandResult::Error(format!("Failed to resume: {}", e)),
                            }
                        }
                        _ => CommandResult::Error(format!(
                            "Multiple sessions match '{}'. Please be more specific.",
                            session_id
                        )),
                    }
                }
                Err(e) => CommandResult::Error(format!("Failed to list sessions: {}", e)),
            }
        }

        "/model" => {
            if parts.len() < 2 {
                println!("\nCurrent model: {}\n", agent.model());
                return CommandResult::Continue;
            }
            let model = parts[1];
            match agent.set_model(model) {
                Ok(()) => {
                    println!("\nSwitched to model: {}\n", model);
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Failed to switch model: {}", e)),
            }
        }

        "/compact" => match agent.compact_session().await {
            Ok((before, after)) => {
                println!("\nSession compacted. Token count: {} → {}\n", before, after);
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to compact: {}", e)),
        },

        "/clear" => {
            agent.clear_session().await;
            println!("\nSession cleared.\n");
            CommandResult::Continue
        }

        "/new" => {
            // Save current session to memory before starting new one
            match agent.save_session_to_memory().await {
                Ok(Some(path)) => {
                    println!("\nSession saved to: {}", path.display());
                }
                Ok(None) => {} // No messages to save
                Err(e) => {
                    eprintln!("Warning: Failed to save session to memory: {}", e);
                }
            }

            match agent.new_session().await {
                Ok(()) => {
                    println!("New session started. Memory context reloaded.\n");
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Failed to create new session: {}", e)),
            }
        }

        "/memory" => {
            if parts.len() < 2 {
                return CommandResult::Error("Usage: /memory <query>".into());
            }
            let query = parts[1..].join(" ");
            match agent.search_memory(&query).await {
                Ok(results) => {
                    if results.is_empty() {
                        println!(
                            "\nNo results found for '{}'. Try /reindex to rebuild memory index.\n",
                            query
                        );
                    } else {
                        println!("\nMemory search results for '{}':", query);
                        for (i, result) in results.iter().enumerate() {
                            let snippet = extract_snippet(&result.content, &query, 120);
                            println!(
                                "{}. [{}:{}] {}",
                                i + 1,
                                result.file,
                                result.line_start,
                                snippet
                            );
                        }
                        println!();
                    }
                    CommandResult::Continue
                }
                Err(e) => CommandResult::Error(format!("Memory search failed: {}", e)),
            }
        }

        "/reindex" => match agent.reindex_memory().await {
            Ok((files, chunks, embedded)) => {
                if embedded > 0 {
                    println!(
                        "\nMemory index rebuilt: {} files, {} chunks, {} embeddings\n",
                        files, chunks, embedded
                    );
                } else {
                    println!(
                        "\nMemory index rebuilt: {} files, {} chunks\n",
                        files, chunks
                    );
                }
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to reindex: {}", e)),
        },

        "/save" => match agent.save_session().await {
            Ok(path) => {
                println!("\nSession saved to: {}\n", path.display());
                CommandResult::Continue
            }
            Err(e) => CommandResult::Error(format!("Failed to save session: {}", e)),
        },

        "/status" => {
            let status = agent.session_status().await;
            println!("\nSession Status:");
            println!("  ID: {}", status.id);
            println!("  Model: {}", agent.model());
            println!("  Messages: {}", status.message_count);
            println!("  Context tokens: ~{}", status.token_count);
            println!("  Compactions: {}", status.compaction_count);

            println!("\nMemory:");
            println!("  Chunks: {}", agent.memory_chunk_count().await);
            if agent.has_embeddings() {
                println!("  Embeddings: enabled");
            }

            if status.api_input_tokens > 0 || status.api_output_tokens > 0 {
                println!("\nAPI Usage:");
                println!("  Input tokens: {}", status.api_input_tokens);
                println!("  Output tokens: {}", status.api_output_tokens);
                println!(
                    "  Total tokens: {}",
                    status.api_input_tokens + status.api_output_tokens
                );
            }
            println!();
            CommandResult::Continue
        }

        "/models" => {
            println!("\nAvailable model prefixes:");
            println!(
                "  claude-cli/*    - Use Claude CLI (e.g., claude-cli/opus, claude-cli/sonnet)"
            );
            println!("  gpt-*           - OpenAI (requires API key)");
            println!("  claude-*        - Anthropic API (requires API key)");
            println!("  ollama/*        - Ollama local (e.g., ollama/llama3)");
            println!("  <other>         - Defaults to Ollama");
            println!("\nCurrent model: {}", agent.model());
            println!("Use /model <name> to switch.\n");
            CommandResult::Continue
        }

        "/context" => {
            let (used, usable, total) = agent.context_usage().await;
            let pct = (used as f64 / usable as f64 * 100.0).min(100.0);
            println!("\nContext Window:");
            println!("  Used: {} tokens ({:.1}%)", used, pct);
            println!("  Usable: {} tokens", usable);
            println!("  Total: {} tokens", total);
            println!("  Reserve: {} tokens", total - usable);

            if pct > 80.0 {
                println!("\n⚠ Context nearly full. Consider /compact or /new.");
            }
            println!();
            CommandResult::Continue
        }

        "/export" => {
            let markdown = agent.export_markdown().await;
            if parts.len() >= 2 {
                let path = parts[1..].join(" ");
                let expanded = shellexpand::tilde(&path).to_string();
                match tokio::fs::write(&expanded, &markdown).await {
                    Ok(()) => {
                        println!("\nSession exported to: {}\n", expanded);
                        CommandResult::Continue
                    }
                    Err(e) => CommandResult::Error(format!("Failed to export: {}", e)),
                }
            } else {
                // Print to stdout
                println!("\n{}", markdown);
                CommandResult::Continue
            }
        }

        _ => {
            // Check if it's a skill command
            if let Some(invocation) = parse_skill_command(input, skills) {
                // Find the skill to get its path
                if let Some(skill) = skills.iter().find(|s| s.name == invocation.skill_name) {
                    let skill_prompt = if invocation.args.is_empty() {
                        format!(
                            "Use the skill at {}. Read it first, then follow its instructions.",
                            skill.path.display()
                        )
                    } else {
                        format!(
                            "Use the skill at {} with this request: {}\n\nRead the skill file first, then follow its instructions.",
                            skill.path.display(),
                            invocation.args
                        )
                    };
                    println!(
                        "\nInvoking skill: {} {}",
                        skill.name,
                        skill.emoji.as_deref().unwrap_or("")
                    );
                    return CommandResult::SendMessage(skill_prompt);
                }
            }

            CommandResult::Error(format!(
                "Unknown command: {}. Type /help for commands.",
                cmd
            ))
        }
    }
}
