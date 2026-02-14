```
                  o         .      o     .        o          .      o      .
                   ᛏᚷᚱᛟᛏᛏᛟᛋᚨᚱᚢᚷᚱᛟᛏᛏᛟᛋᚨᚱᚢᚷᚱᛟᛏᛏᛟᛋᛋᛟᛏᛏᛟᚱᚷᚢᚱᚨᛋᛟᛏᛏᛟᚱᚷᚢᚱᚨᛋᛟᛏ
            ╔════════════════════════════════════════════════════════════════════╗
            ║ ░░░░ ▒▒▒ ▓▓▓ █ █ █ [  K E R N E L : O N  ] █ █ █ ▓▓▓ ▒▒▒ ░░░░░     ║
            ║ ░▒▓ ◢◤                                                       ◥◣ ▓▒░║
            ║ ░▒▓ █     ███████╗  ██╗  ███████╗  ██████╗         ▲         █ ▓▒░ ║
            ║ ░▒▓ █     ╚══███╔╝  ██║  ██╔════╝  ██╔══██╗       ▲ ▲        █ ▓▒░ ║
            ║ ░▒▓ █       ███╔╝   ██║  █████╗    ██████╔╝      ▲ ▲ ▲       █ ▓▒░ ║
            ║ ░▒▓ █      ███╔╝    ██║  ██╔══╝    ██╔══██╗     ▀▀▀█▀▀▀      █ ▓▒░ ║
            ║ ░▒▓ █     ███████╗  ██║  ███████╗  ██║  ██║        █         █ ▓▒░ ║
            ║ ░▒▓ █     ╚══════╝  ╚═╝  ╚══════╝  ╚═╝  ╚═╝        ▼         █ ▓▒░ ║
            ║ ░▒▓ ◥  ◣     :: ᚱ ᛟ ᚷ :: ᛟ ᚱ ᛏ :: ᚨ ᛋ ᚢ :: ᚱ ᛟ ᚷ ::        ◤  ▒░║
            ║ ░░░░ ▒▒▒ ▓▓▓ █ █ █ ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀ █ █ █ ▓▓▓ ▒▒▒ ░░░░░     ║
            ╚════════════════════════════════════════════════════════════════════╝
                     ᚨᚠᚢᚦᚨᚱᚲᚷᚱᛟᛏᛏᛟᛋᚨᚱᚢᚠᚦᚨᚱᚲᚷᚱᛟᛏᛏᛟᛟᛏᛏᛟᚱᚷᚲᚱᚨᚦᚠᚢᚱᚨᛋᛟᛏᛏᛟᚱᚷᚲ
                  °   .    o    .   °    .   o    .    °    .   o    .   °
```

**A local‑first cognitive kernel for your personal AI staffer.**  

Zier Alpha is the foundation of **Vizier** — a silicon‑based assistant that works *for* you, not *as* you. It enforces a strict separation between reasoning and execution, runs entirely on your device, and persists knowledge across sessions using plain markdown files.
For a detailed technical overview, see [ARCHITECTURE.md](ARCHITECTURE.md).

## Core Philosophy

- **Single binary** – no Node.js, Python, or Docker runtimes. Embedded Deno for extensions.
- **Data sovereignty** – your memory never leaves localhost unless you explicitly allow it.
- **Persistent memory** – hybrid (FTS + vector) search over markdown notes.
- **Autonomous heartbeat** – scheduled tasks, system monitoring, and proactive audits.
- **Trust‑aware ingress** – distinguishes owner commands from untrusted events.
- **Secure sandbox** – tools run in isolated environments (Apple Sandbox on macOS).
- **Extensible** – write your own tools in TypeScript/JavaScript using the embedded Deno runtime.

---

## Quick Start

### Installation

```bash
# Clone and build
git clone https://github.com/zier-alpha-app/zier-alpha.git
cd zier-alpha

# Full install (includes desktop GUI)
cargo install --path .

# Headless install (server/daemon only)
cargo install --path . --no-default-features
```

### Initialize configuration and workspace

```bash
zier-alpha config init
```

### Start a chat

```bash
zier-alpha chat
```

### Ask a single question

```bash
zier-alpha ask "Summarize the latest HN top stories"
```

### Run as a background daemon

```bash
zier-alpha daemon start
```

---

## Key Concepts

- **Kernel** – the core Rust process that manages cognition, memory, and tools.
- **Workspace** – a directory (default `~/.zier-alpha/workspace/`) containing all long‑term memory files.
- **Memory files** – markdown files that the agent reads and writes to persist knowledge:
  - `MEMORY.md` – curated facts, preferences, decisions.
  - `HEARTBEAT.md` – pending tasks for autonomous execution.
  - `SOUL.md` – persona and tone guidance.
  - `IDENTITY.md`, `USER.md`, `AGENTS.md`, `TOOLS.md` – OpenClaw‑compatible context files.
  - `memory/YYYY-MM-DD.md` – daily session logs.
- **Skills** – `SKILL.md` files placed in `skills/` that teach the agent new abilities. They can be slash‑command invocable.
- **Extensions** – JavaScript/TypeScript programs that register new tools using the `pi` and `zier` globals.

---

## Configuration

All settings are stored in `~/.zier-alpha/config.toml`.  
You can also use environment variables:

- `ZIER_ALPHA_CONFIG` – override config file path.
- `ZIER_ALPHA_WORKSPACE` – override workspace directory.
- `ZIER_ALPHA_PROFILE` – select a profile (e.g., `work` → `~/.zier-alpha/workspace-work`).
- `ZIER_ALPHA_AGENT` – set the agent ID (default `main`).
- `ZIER_ALPHA_DISABLE_DISK_MONITOR` – set to `1` to disable automatic disk space checks (useful for CI or tests).

See [`config.example.toml`](./config.example.toml) for a complete reference with comments.

---

## Modes of Operation

### 1. Interactive CLI (`zier-alpha chat`)
Full conversational mode with streaming responses, tool execution, and session management.

### 2. Single‑shot CLI (`zier-alpha ask`)
Ask one question and get a text (or JSON) answer. Useful for scripting or integration.

### 3. Desktop GUI (`zier-alpha desktop`)
Native egui application that embeds the agent directly – no HTTP server required.

### 4. Daemon Mode (`zier-alpha daemon`)
Background process that enables:
- **Heartbeat** – periodic autonomous tasks.
- **HTTP server** – REST API and WebSocket for external UIs.
- **Telegram integration** – polling or webhook.
- **Scheduler** – cron‑like jobs.
- **Ingress bus** – unified event handling.

---

## Extensions

Zier Alpha ships with two built‑in extensions, but you can create your own.

### Hive – Subagent Orchestration
Define specialized agents (markdown files) that the main agent can delegate tasks to. Hive manages process forking, session hydration, and IPC.

### tmux_bridge – Persistent Background Processes
Spawn, monitor, and control long‑running processes using `tmux` as a backend. Includes tools for inspecting logs, sending keystrokes (`expect`), and pattern monitoring.

---

## Documentation

- [Architecture Overview](./docs/ARCHITECTURE.md) – data flow, trust model, memory system, concurrency, security.
- [Configuration Reference](./config.example.toml) – all options explained.
- [Extension API](./docs/EXTENSION_API.md) *(coming soon)* – writing your own tools in JavaScript.

---

## License

Apache 2.0
```

**docs/ARCHITECTURE.md**

```markdown
# Zier Alpha Architecture

This document describes the internal architecture of Zier Alpha – a local‑first, secure, and autonomous AI assistant.  
The design follows the **VIZIER** principles:

- **V** – Verified ingress (trust levels)
- **I** – Isolated execution (sandboxed tools)
- **Z** – Zero‑latency memory (hybrid search)
- **I** – Intelligent scheduling (heartbeat & cron)
- **E** – Extensible runtime (Deno)
- **R** – Reliable persistence (OpenClaw‑compatible sessions)

---

## High‑level Overview

```mermaid
graph TD
    subgraph Ingress Sources
        CLI[CLI / Desktop]
        HTTP[HTTP API]
        WS[WebSocket]
        TG[Telegram]
        Cron[Scheduler]
    end

    Ingress -->|IngressMessage| Bus[Ingress Bus]

    Bus --> ControlPlane[Control Plane Loop]

    ControlPlane -->|Trust Check| Routing{Trust Level}

    Routing -->|OwnerCommand| RootAgent[Root Agent<br/>(full tools)]
    Routing -->|TrustedEvent| JobAgent[Job Agent<br/>(scoped tools)]
    Routing -->|UntrustedEvent| Sanitizer[Sanitizer Agent<br/>(no tools)]

    RootAgent -->|execute| Tools
    JobAgent -->|execute| Tools

    subgraph Tools
        Builtin[Built‑in Rust Tools]
        Script[Deno Script Tools]
        MCP[MCP Servers]
    end

    Tools -->|result| Artifacts[Artifact Storage<br/>(markdown + YAML)]

    RootAgent -->|read/write| Memory[Memory Manager]
    JobAgent -->|read| Memory
    Sanitizer -->|read| Memory

    Memory -->|index| SQLite[(SQLite DB)]
    Memory -->|files| Workspace[Workspace Files<br/>(MEMORY.md, etc.)]

    Heartbeat[Heartbeat Runner] -->|poll| Memory
    Heartbeat -->|ingress| Bus

    Scheduler[Scheduler] -->|cron| Bus

    Extensions[Deno Extensions] -->|register tools| Script
```

---

## Data Flow

### 1. Ingress

All inputs are normalized into an `IngressMessage` containing:
- `source` – e.g., `telegram:12345`, `scheduler:daily`, `http:session-abc`
- `payload` – the textual content (or a command)
- `trust` – one of `OwnerCommand`, `TrustedEvent`, `UntrustedEvent`
- `images` – optional base64‑encoded image attachments

**Trust assignment**:
- Telegram messages from the configured `owner_telegram_id` → `OwnerCommand`
- Scheduler jobs and internal scripts → `TrustedEvent`
- All other sources (webhooks, non‑owner Telegram) → `UntrustedEvent`

### 2. Ingress Bus

A Tokio MPSC channel that decouples input sources from processing.  
Multiple producers can push messages; a single consumer (the control plane loop) processes them sequentially (each in its own task).

### 3. Control Plane

The control loop reads from the bus and spawns a new asynchronous task for each message.  
Inside the task:

- **Trust check** determines which agent to use.
- **Session resolution** – messages from the same source reuse an in‑memory session (cached in `GlobalSessionManager`).
- **Agent instantiation** – a prototype agent is cloned and then configured with the appropriate context strategy and tool set.

#### Context Strategies

- `Full` – session persists across turns, memory files are loaded.
- `Stateless` – fresh session for each turn (used for jobs and sanitizer).
- `Episodic` – reserved for future use (e.g., summarization tasks).

### 4. Agent Lifecycle

An `Agent` contains:
- A `Session` (in‑memory conversation history)
- A `SmartClient` (LLM provider router with fallbacks)
- A `MemoryManager` handle
- A vector of `Arc<dyn Tool>`
- Compaction strategy and cumulative token usage

**Agent steps**:
1. If the session is new, build the system prompt (identity, safety rules, available tools, memory context).
2. Append the user message (with images) to the session.
3. Optionally run a **pre‑compaction memory flush** if the token count approaches the context limit.
4. Call the LLM (streaming or non‑streaming) with the current tool schemas.
5. If the response contains tool calls, execute them sequentially, append tool results, and loop back to step 4.
6. Append the final assistant message and save the session (auto‑save).

---

## Memory System

The `MemoryManager` is the single source of truth for long‑term memory.

### Workspace Files

- `MEMORY.md` – curated, persistent facts.
- `HEARTBEAT.md` – pending tasks for autonomous mode.
- `SOUL.md` – persona and tone.
- `IDENTITY.md`, `USER.md`, `AGENTS.md`, `TOOLS.md` – OpenClaw‑compatible context.
- `memory/YYYY-MM-DD.md` – daily session logs.
- `skills/*/SKILL.md` – user‑defined skills.

### Indexing

All markdown files under the workspace (and optionally external paths) are chunked (configurable chunk size/overlap) and stored in an SQLite database with:

- `files` – tracks file hashes and modification times.
- `chunks` – stores each chunk’s text, line range, and (optional) embedding.
- `chunks_fts` – FTS5 virtual table for keyword search.
- `embedding_cache` – caches embeddings by provider and content hash.

### Search

Two modes:

- **FTS only** – fast, no external dependencies.
- **Hybrid** – if an embedding provider is configured (`local`, `openai`, `gguf`), the query is embedded and combined with FTS results using a weighted rank‑based scoring (`text_weight` and `vector_weight` are currently hard‑coded but configurable in future versions).

Embeddings are generated asynchronously in batches and cached to avoid recomputation.

---

## Tools

Tools implement the `Tool` trait with a JSON schema (for the LLM) and an `execute` method.

### Built‑in Tools

- `bash` – run shell commands (with timeout, CWD confinement).
- `read_file` / `write_file` / `edit_file` – file operations with cognitive routing.
- `memory_search` / `memory_get` – search and retrieve memory snippets.
- `web_fetch` – fetch a URL (with size limit).

### Script Tools

Extensions written in JavaScript/TypeScript can register tools using `pi.registerTool()`.  
The `ScriptService` runs a dedicated Deno runtime in a separate OS thread. Communication is via `mpsc` and `oneshot` channels.

### MCP Tools

The **Model Context Protocol** manager can spawn and communicate with external MCP servers. Tools exposed by those servers become available to the agent.

### Content Sanitization

Tool outputs can be wrapped in XML‑style delimiters (`<tool_output>...</tool_output>`) and truncated. Suspicious patterns (e.g., “ignore previous instructions”) are detected and logged.

---

## Trust and Security

### Trust Levels

- **OwnerCommand** – full access to all tools, can read/write any file (subject to workspace/project boundaries).
- **TrustedEvent** – scoped access defined by the job configuration (e.g., only certain tools).
- **UntrustedEvent** – no tools, routed to a sanitizer agent that only summarises the input.

### Sandboxing

- **macOS**: Apple Sandbox profiles are generated dynamically and applied via `sandbox-exec`.
- **Linux/Windows**: planned (bubblewrap, AppContainer). Currently a warning is logged and the tool runs unsandboxed.

### Deno Op Security

Each Deno op performs permission checks against the `SandboxPolicy` (allow_read, allow_write, allow_network, allow_env). Paths are canonicalized and must reside inside allowed directories.

### Command Safety

The `op_zier_exec` op enforces:

- **CWD confinement** – the working directory must be inside the project or workspace (or `/tmp` for temporary operations).
- **Shell chaining** – `&&`, `||`, `;`, `` ` `` are blocked by default.
- **Hard‑blocked commands** – `rm -rf /`, `mkfs`, `dd`, fork bombs are rejected.
- **Approval‑required commands** – e.g., `terraform destroy`, `aws delete` – these require user approval (configurable).

---

## Concurrency

Two mechanisms prevent data races and resource contention:

- **`TurnGate`** – in‑process semaphore that serialises agent turns inside the daemon. Heartbeat tries to acquire non‑blocking; HTTP requests wait.
- **`WorkspaceLock`** – cross‑process advisory file lock (`workspace.lock`) that guards the workspace directory. All CLI commands and the daemon acquire this lock before modifying any file.

---

## Persistence

### Sessions

Sessions are saved as JSONL files in `~/.zier-alpha/agents/<agent>/sessions/<uuid>.jsonl`. The format is compatible with **OpenClaw**:

- First line: session header (`type: "session"`, `version`, `id`, `timestamp`, `cwd`).
- Subsequent lines: messages (`type: "message"`) with role, content (as an array of content parts), tool calls, metadata.

### Session Store

`sessions.json` in the same directory tracks per‑session metadata:

- CLI session IDs for `claude-cli` provider (to resume conversations across restarts)
- Heartbeat deduplication state
- Token usage (optional)

### Auto‑save

Active sessions are saved every 5 minutes and on every turn (if dirty). The `GlobalSessionManager` runs a background task that periodically flushes dirty sessions.

---

## Extensions

Extensions are JavaScript files placed in `~/.zier-alpha/extensions/` (or loaded from a configured path). They are loaded at startup and can:

- Register tools (using `pi.registerTool`)
- Provide status lines (using `zier.hooks.on_status`)
- Schedule recurring jobs (using `zier.scheduler.register`)
- Push ingress events (using `zier.ingress.push`)
- Interact with MCP servers (using `zier.mcp`)

The Hive and tmux_bridge extensions are examples of what can be built.

---

## Configuration

See [`config.example.toml`](../config.example.toml) for a complete reference.  
Key sections:

- `[agent]` – default model, context window, token reserve.
- `[providers]` – API keys and endpoints for OpenAI, Anthropic, Ollama, Claude CLI, and any custom OpenAI‑compatible provider (e.g., openrouter, together). Additional provider sections are accepted.
- `[models]` – custom model definitions with inheritance and fallback chains.
- `[heartbeat]` – enable/disable, interval, active hours.
- `[memory]` – workspace path, embedding provider, chunking parameters.
- `[server]` – HTTP server settings, Telegram integration.
- `[tools]` – timeouts, approval list, content sanitization.
- `[workdir]` – overlay/mount strategy for cognitive vs. project files.
- `[extensions.hive]` – Hive subagent configuration.
- `[extensions.mcp]` – MCP server definitions.

---

## Future Directions

- **Full streaming tool execution** – true incremental delivery of tool calls.
- **User approval UI** – native prompts in desktop and web UIs.
- **Linux sandboxing** – bubblewrap/nsjail integration.
- **Plugin marketplace** – share extensions via a simple registry.
- **Memory graph** – relationship extraction and linking between memory chunks.

---

*Last updated: 2026‑02‑14*