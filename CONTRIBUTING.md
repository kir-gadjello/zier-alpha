# CONTRIBUTING.md – Zier Alpha Architecture & Development Guide

## Mission
Zier Alpha is a **local‑first cognitive kernel** for a personal AI staffer.  
It runs as a single binary, enforces strict separation between reasoning and execution, persists knowledge in plain markdown, and is extensible via embedded Deno.

## Core Principles
- **Data sovereignty** – all memory stays on your device.
- **Trust‑aware ingress** – inputs tagged as `OwnerCommand`, `TrustedEvent`, or `UntrustedEvent`.
- **Isolated execution** – tools run in sandboxes (Apple Sandbox on macOS, bubblewrap planned for Linux).
- **Hybrid memory** – keyword (FTS) + optional semantic search (embeddings).
- **Extensible** – extensions (TypeScript/JavaScript) register tools, status hooks, and cron jobs.

## Architecture at a Glance
```
Ingress (CLI, HTTP, Telegram, scheduler, heartbeat)
    │
    ▼
IngressBus (MPSC channel)
    │
    ▼
Control plane loop – per message spawn task:
    ├─ resolve session (GlobalSessionManager)
    ├─ clone prototype Agent
    ├─ build system prompt (memory context + skills + status)
    └─ run Agent turn (ChatEngine)
```
- **Agent** – owns `Session` (conversation history), `SmartClient` (LLM provider with fallback), `ToolExecutor`, and `MemoryManager`.
- **Memory** – SQLite with FTS5, optional embeddings via fastembed/OpenAI/GGUF. Files: `MEMORY.md`, `SOUL.md`, `HEARTBEAT.md`, daily logs in `memory/`.
- **Tools** – built‑in (bash, file ops, web fetch), Deno‑registered, MCP servers.
- **Concurrency** – `TurnGate` (in‑process semaphore) + `WorkspaceLock` (cross‑process file lock) serialise all agent turns.
- **Extensions** – load from `~/.zier-alpha/extensions/*.js`; can call `pi.registerTool`, `zier.scheduler.register`, `zier.ingress.push`.

## Key Files & Directories
- `src/agent/` – core agent logic, providers, tools.
- `src/memory/` – indexing, embeddings, file watcher.
- `src/scripting/` – Deno runtime, capability sandboxing.
- `extensions/` – built‑in extensions (Hive, tmux_bridge).
- `config.example.toml` – all configuration options explained.
- `CHANGELOG.md` – tracks recent fixes (critical for understanding current behaviour).
- `Cargo.toml` – features: `desktop` (egui), `fastembed` (local embeddings), `gguf` (llama.cpp).

## Development Constraints & Advice

### 1. Never Break Backward Compatibility
- Existing user workflows (CLI, config files, session format) **must** continue working.
- If a breaking change is unavoidable, update the changelog and provide a migration path.

### 2. Respect Trust Levels
- `OwnerCommand` – full tool access, used for authenticated users (Telegram owner, local CLI).
- `TrustedEvent` – scoped tools (defined in job config). Used for scheduler, scripts.
- `UntrustedEvent` – **no tools**, routed to a sanitizer agent. Never allow any tool execution.

### 3. Concurrency Is Non‑Negotiable
- Always acquire `WorkspaceLock` (blocking) **before** any file write/modification. Use `spawn_blocking` in async contexts.
- Use `TurnGate::acquire()` for agent turns; heartbeat uses `try_acquire()` to skip if busy.
- Never hold a lock across an await point – this can deadlock the async runtime.

### 4. Tools Must Be Safe
- Built‑in tools (`bash`, file ops) should eventually use the same safety checks as Deno’s `op_zier_exec` (`SafetyPolicy`).
- For tools that require approval, the flow must work in **all interfaces** (CLI, desktop, HTTP, OpenAI proxy). Currently HTTP lacks approval – this must be fixed.
- Deno extensions **must** declare capabilities (`// @capability read=...`) – the policy checks them against the configured `SandboxPolicy`.

### 5. Memory & Embeddings
- New files are watched and reindexed automatically (watcher runs in daemon mode).
- Embedding generation is **not** automatic – call `memory.generate_embeddings()` after indexing to populate semantic search.
- Hybrid search weights are hard‑coded (0.3 FTS, 0.7 vector) – future work should make them configurable.

### 6. Testing
- Unit tests for providers, sanitisation, concurrency.
- Integration tests for end‑to‑end flows (tests/e2e.rs) – they currently rely on a mock provider; ensure they pass in CI.
- Security tests (injection, sandbox) are in `tests/injection.rs`, `tests/sandbox.rs`.

### 7. Common Pitfalls to Avoid
- **Blocking I/O in async code** – use `tokio::fs`, not `std::fs`.
- **Inconsistent path resolution** – always use `resolve_path` from `tools.rs` (soon to be moved to a common module).
- **Approval handling in non‑streaming mode** – if a tool requires approval, the non‑streaming `chat()` will return an error. This is undesirable – either implement a synchronous approval mechanism or convert to streaming.
- **Forgetting to mark session `dirty`** – after any modification to the session (messages, metadata), set `dirty = true` so auto‑save persists it.
- **Environment variable overrides** – when setting env for child processes, block dangerous vars (PATH, HOME, LD_*). Use the blacklist in `op_zier_exec`.

### 8. Extensibility
- Extensions are powerful – they can register tools, schedule cron jobs, and push ingress messages. This capability **must** be gated by user consent (e.g., a confirmation prompt on first load). Currently it’s implicit – fix this.
- The `on_status` hook allows extensions to add lines to the system prompt (shown in desktop status). Keep these concise.

### 9. Deno Runtime – Avoiding Deadlocks

#### Async Ops
All custom Deno operations that perform I/O **must** be defined with `#[op2(async)]` and use `tokio::fs` APIs. Never use `std::fs` inside an op, as it will block the V8 thread and cause hangs.

#### Promise Resolution – Do Not Use `JsRuntime::resolve`
**Critical:** When awaiting a promise from within an async op on a `current_thread` tokio runtime, **never call** `JsRuntime::resolve(promise).await`. This method internally may re‑enter the event loop and deadlock.

Instead, use the **manual polling pattern**:

```rust
let promise_global = self.runtime.execute_script("<tool_exec>", code)?;

loop {
    let state = {
        let scope = &mut self.runtime.handle_scope();
        let promise_local = v8::Local::new(scope, &promise_global);
        let promise = v8::Local::<v8::Promise>::try_from(promise_local)?;
        match promise.state() {
            v8::PromiseState::Pending => None,
            v8::PromiseState::Fulfilled => {
                let value = promise.result(scope);
                Some(Ok(v8::Global::new(scope, value)))
            }
            v8::PromiseState::Rejected => {
                let exception = promise.result(scope);
                let msg = exception.to_rust_string_lossy(scope);
                Some(Err(anyhow::anyhow!("Promise rejected: {}", msg)))
            }
        }
    };

    match state {
        Some(Ok(global)) => break global,
        Some(Err(e)) => return Err(e.into()),
        None => self.runtime.run_event_loop(Default::default()).await?,
    }
}
```

This pattern drives the event loop only when the promise is pending, avoiding nested `run_event_loop` calls that cause deadlocks. See `src/scripting/deno.rs` (`execute_tool` and `get_status`) for working examples.

#### Path Validation
- **Never perform I/O** in `check_path` or similar sandbox checks.
- Use pure path manipulation (`resolve_path`, prefix checks) **without** `exists()`, `canonicalize()`, or other syscalls.
- Capability checks should operate on the *requested* path's prefix relationship to allowed roots regardless of file existence.

Violating these rules will reintroduce blocking and/or deadlocks.

## Current Priorities (from Changelog & audit)
- **Complete tool approval flow for HTTP/OpenAI proxy** – without this, remote clients cannot use tools requiring approval.
- **Apply uniform safety checks** – make built‑in tools as safe as Deno scripts.
- **Automatic embedding generation** – users shouldn’t need to manually reindex.
- **Linux sandboxing** – replace `unshare` with bubblewrap.
- **Secrets management** – store API keys in OS keychain, not plaintext config.

When in doubt, consult the `CHANGELOG.md` for recent fixes and the audit report for deeper insights.