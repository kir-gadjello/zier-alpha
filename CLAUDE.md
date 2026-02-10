
# CLAUDE.md

**Zier-Alpha** is a local-first **Cognitive Kernel** and process orchestrator. It implements the **VIZIER** architecture: a strictly typed, trust-aware loop that decouples input (Ingress) from reasoning (Agent), enforced by a secure Control Plane.

## Development Directives

### Build & Run
```bash
cargo build --release            # Core binary (~27MB)
cargo run -- chat                # Interactive REPL
cargo run -- daemon start        # Start Kernel (Ingress + Scheduler + HTTP)
cargo run -- ask "query"         # One-shot query

```

### Testing

**Crucial:** This project relies heavily on integration tests to verify the Deno-Rust-Tmux bridge.

```bash
cargo test                       # Unit tests
cargo test --test e2e            # End-to-End integration
cargo test --test tmux_bridge    # Validate process orchestration & confinement

```

## Architecture: VIZIER

The system follows a strict unidirectional flow:
`Ingress Source` → `IngressBus` → `Control Plane` → `Ephemeral Agent` → `Action/Artifact`

1. **Ingress Layer** (`src/ingress/`):
* Normalizes inputs (Telegram, HTTP, Cron) into `IngressMessage`.
* **Non-blocking:** Pushes to `mpsc::channel` immediately.


2. **Control Plane** (`src/ingress/controller.rs`):
* **The Gatekeeper:** Routes messages based on `TrustLevel`.
* `OwnerCommand`: Full tool access (Shell, File IO).
* `TrustedEvent`: Scoped access (defined by Job config).
* `UntrustedEvent`: Sandbox only (Sanitizer agent, no tools).


3. **Execution** (`src/agent/`):
* **Stateless Core:** `Agent` struct is rebuilt per request.
* **Stateful Session:** Context persists in `Session` (JSONL).
* **Active Memory:** Agent explicitly "flushes" context to Markdown before context window compaction.



## Core Subsystems

### 1. Memory ("Active Source of Truth")

* **Location:** `src/memory/`
* **Philosophy:** Markdown files (`MEMORY.md`, `heartbeat/*.md`) are the database. SQLite is just a disposable index.
* **Search:** Hybrid (FTS5 + Vector/FastEmbed).
* **Constraint:** `rusqlite` is blocking. **Always** wrap DB ops in `task::spawn_blocking` to avoid stalling the async runtime.

### 2. Scripting & Plugins (`tmux_bridge`)

* **Location:** `src/scripting/`
* **Runtime:** Embedded Deno (V8).
* **Capabilities:**
* `zier.os.exec`: Spawns subprocesses (strictly confined to `workspace_dir` unless configured).
* `zier.ingress.push`: Injects events back into the Control Plane (e.g., monitoring alerts).


* **Safety:**
* **CWD Jail:** Operations outside workspace are blocked by Rust core.
* **Heuristic Blocker:** Destructive commands (`rm -rf`, `mkfs`) require explicit user approval via `StreamEvent`.



### 3. Process Orchestration (`tmux`)

* The **Tmux Bridge** plugin transforms Zier into a dev daemon.
* **Socket Isolation:** Uses `tmux -L zier` to protect user sessions.
* **LLM Hygiene:**
* **Search-First:** Prefers `grep`/`tail` over dumping full history.
* **XML Armoring:** All process output is wrapped in `<stdout>`/`<stderr>` tags to prevent prompt injection from logs.
* **Peripheral Vision:** Running processes automatically inject status summaries into the System Prompt.



## Coding Standards

1. **Async Hygiene:** The Control Plane is a single-threaded event loop. **Never block it.** Offload heavy IO/DB to worker threads.
2. **Output Format:** Tools must return structured XML/JSON. Raw text confuses the model during complex multi-step reasoning.
* *Bad:* `file not found`
* *Good:* `<error code="404">File 'src/main.rs' not found in cwd</error>`


3. **Error Handling:** Do not panic on Ingress. Log error, push `Artifact` (failure report), and return to loop.
4. **Deps:** Keep `Cargo.toml` lean. Features `fastembed` and `gui` are optional.

## Configuration

* **Path:** `~/.zier-alpha/config.toml`
* **Env Vars:** Supports `${VAR}` expansion.
* **Key Sections:**
* `[agent]`: Model selection, context window.
* `[scripting]`: `allow_shell`, `allow_global_cwd` (Safety gates).
* `[server]`: Telegram/HTTP binding.

