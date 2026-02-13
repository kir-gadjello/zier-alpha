# Zier Alpha Architecture

Zier Alpha is a local-first AI assistant designed as a "silicon-based staffer". It is a single-binary application that combines an LLM client, a persistent memory system, a tool execution engine, and a secure scripting environment.

## Core Components

The system is built around the **VIZIER** pattern (Verified Ingress, Zero-trust Internal Execution Router).

### Agent Decomposition

The `Agent` struct (`src/agent/mod.rs`) has been refactored from a monolith into four focused components:

1.  **ChatEngine** (`src/agent/chat_engine.rs`): Orchestrates the LLM interaction loop. It handles:
    *   Sending messages to the provider (OpenAI, Anthropic, etc.).
    *   Streaming responses token-by-token.
    *   Detecting and parsing tool calls.
    *   Managing the conversation turn loop (LLM -> Tool -> LLM).

2.  **SessionManager** (`src/agent/session_manager.rs`): Manages conversation state.
    *   Loads and saves sessions (JSONL format, OpenClaw-compatible).
    *   Tracks token usage using `tiktoken-rs`.
    *   Handles session compaction (summarization) to fit within context windows.
        *   Supports configurable fallback models if the primary model fails during compaction (e.g., due to context overflow).
    *   Implements "dirty" tracking for optimized auto-saves.

3.  **ToolExecutor** (`src/agent/tool_executor.rs`): securely executes tools.
    *   Dispatches calls to the appropriate tool implementation.
    *   Enforces the `ApprovalManager` check (requiring user confirmation for sensitive tools).
    *   Sanitizes tool output (truncation, error handling).

4.  **MemoryContextBuilder** (`src/agent/memory_context.rs`): Constructs the system prompt.
    *   Injects static context (identity, rules).
    *   Injects dynamic memory (workspace files like `MEMORY.md`, `SOUL.md`).
    *   Injects retrieval-augmented generation (RAG) results from `MemoryIndex`.

### Tool System

Tools implement the `Tool` trait (`src/agent/tools/mod.rs`). There are four types of tools:

1.  **Built-in Tools**: Native Rust implementations (e.g., `read_file`, `write_file`, `bash`, `web_fetch`).
2.  **Script Tools**: JavaScript/TypeScript functions executed in the embedded Deno runtime.
3.  **External Tools**: Standalone executables configured in `config.toml` (e.g., `git`, `docker`). These run in an OS-level sandbox.
4.  **MCP Tools**: Tools dynamically discovered from Model Context Protocol (MCP) servers.

#### Security & Sandboxing

Zier Alpha employs a "defense-in-depth" security model:

*   **Capability-Based Deno Sandbox**: Script tools must declare required permissions (read/write paths) at load time. The Deno runtime (`src/scripting/deno.rs`) enforces these capabilities for file operations.
*   **OS-Level Sandboxing**: External commands (via `ExternalTool` or Deno's `zier.exec`) are wrapped in OS-specific isolation mechanisms (`src/agent/tools/runner.rs`):
    *   **macOS**: Uses `sandbox-exec` with a dynamically compiled SBPL profile.
    *   **Linux**: Uses namespaces (`unshare`) and `seccomp` filters (via `caps` crate) to restrict filesystem and network access.
*   **Approval Enforcement**: Tools marked as `requires_approval` in config cannot execute until an explicit approval token is granted by the user (via CLI, HTTP, or GUI). This check happens deep in `ToolExecutor`, preventing UI bypasses.

## Memory System

The memory system (`src/memory/`) mimics the OpenClaw structure for compatibility:

*   **Workspace**: A directory containing Markdown files (`MEMORY.md`, `knowledge/*.md`).
*   **MemoryIndex**: A SQLite database (`memory.sqlite`) storing:
    *   File metadata and hashes.
    *   Text chunks for full-text search (FTS5).
    *   Vector embeddings for semantic search (using `sqlite-vec` or `fastembed`).
*   **Connection Pooling**: `MemoryIndex` uses `r2d2` for connection pooling, allowing concurrent search requests while maintaining a single writer for indexing.
*   **Async I/O**: All file operations are asynchronous (using `tokio::fs` or `spawn_blocking`) to prevent blocking the main runtime.

## Configuration

Configuration is loaded from `~/.zier-alpha/config.toml`. The `Config` struct (`src/config/mod.rs`) includes robust validation logic (`validate()`) to catch errors (e.g., missing API keys, invalid paths, circular model inheritance) at startup.

## Testing Strategy

*   **Unit Tests**: Validate individual components (e.g., `config` validation, `memory` indexing).
*   **Integration Tests**: End-to-end tests (`tests/e2e.rs`, `tests/deno_tools.rs`, `tests/mcp_e2e.rs`) verify the interaction of components in a realistic environment.
*   **Sandbox Tests**: `tests/sandbox.rs` verifies that security boundaries are enforced on the host OS.
