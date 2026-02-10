# tmux_bridge Plugin

The `tmux_bridge` plugin enables Zier to spawn, monitor, and control background processes using `tmux` as a backend. This allows processes to persist across agent restarts and provides a robust, observable environment for long-running tasks.

## Features

- **Persistent Processes:** Background jobs run in detached `tmux` sessions.
- **LLM-Optimized Observability:** Logs and status are returned in structured XML for easy parsing.
- **Safety Gating:** Commands are checked against a strict safety policy (CWD confinement, blocked dangerous commands).
- **Monitoring Daemon:** Background daemon scans logs for error patterns and alerts the agent via ingress events.
- **Interactive Automation:** Tools to wait for output patterns and send keystrokes (`expect`).
- **Temporal Reasoning:** Tools to query history and see what changed since the last check (`diff`).

## Tools

### `tmux_spawn`
Starts a new process in a background session.
- `command`: Shell command to run.
- `name`: Unique session ID.
- `cwd`: (Optional) Working directory.

### `tmux_control`
Send input or signals to a running session.
- `id`: Session ID.
- `action`: `write` (send keys), `signal` (send SIGINT), `kill` (terminate session).
- `payload`: Text to write (for `write` action).

### `tmux_inspect`
View logs and status.
- `id`: Session ID.
- `mode`: `tail` (last N lines), `grep` (filter logs), `full_status`.

### `tmux_monitor`
Register a background monitor for regex patterns.
- `id`: Session ID.
- `pattern`: Regex to match.
- `event_message`: Message to send when matched.

### `tmux_history` / `tmux_diff`
Query session event log or see recent changes.

## Security

- **CWD Confinement:** Processes are restricted to the project or workspace directory.
- **Command Blocking:** Destructive commands (`rm -rf`, `mkfs`) are blocked.
- **Environment Safety:** Sensitive environment variables (`PATH`, `HOME`) cannot be overwritten.
- **Shell Chaining:** Chaining operators (`&&`, `|`, `;`) are blocked by default to prevent injection.

## Installation

This plugin is included with Zier Alpha. No additional installation is required. Ensure `tmux` is installed on the host system.
