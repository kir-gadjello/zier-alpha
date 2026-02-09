# Zier-Alpha

A local-first cognitive kernel engineered in Rust. This is the alpha foundation of Vizier, a silicon-based Staffer that enforces a strict separation between cognition and execution. Featuring persistent memory and autonomous agency, it reserves the root context for reasoning while strictly confining tool use to secure, discrete environments.

## Core Philosophy

* **Single binary** — No Node.js, Docker, or Python runtime dependencies for the core.
* **Data sovereignty** — Runs entirely on localhost. Your memory and screen data never leave the device without explicit intent.
* **Persistent memory** — Markdown-based knowledge store with SQLite-backed full-text and semantic search.
* **Autonomous heartbeat** — Background event loop for scheduled tasks and system monitoring.
* **Trust-aware ingress** — Architecturally distinguishes between Owner commands and external events (webhooks, news) to prevent prompt injection.
* **Secure sandbox** — Tools run in isolated environments (Apple Sandbox on macOS) to prevent data exfiltration.

## Installation

Zier-Alpha is currently distributed as source. You will need a Rust toolchain installed.

```bash
# Clone the repository
git clone https://github.com/your-username/zier-alpha.git
cd zier-alpha

# Full install (includes desktop GUI)
cargo install --path .

# Headless install (server/daemon only)
cargo install --path . --no-default-features

```

## Quick Start

Initialize the configuration and workspace:

```bash
zier-alpha config init

```

Start an interactive chat session:

```bash
zier-alpha chat

```

Ask a single question from the CLI:

```bash
zier-alpha ask "Summarize the latest HN top stories"

```

Run as a background daemon (enables Heartbeat and HTTP API):

```bash
zier-alpha daemon start

```

## Architecture

Zier-Alpha implements the VIZIER architecture: a Secure Cognitive Kernel that decouples input sources from execution logic. This ensures that untrusted inputs (like web content or forwarded messages) cannot hijack the agent's tool capabilities.

```mermaid
graph TD
    Ingress[Ingress Sources] -->|IngressMessage| Bus[Ingress Bus]

    subgraph Ingress Sources
        Telegram[Telegram Gateway]
        Cron[Scheduler]
        API[HTTP API]
    end

    Bus --> ControlPlane[Control Plane Loop]

    subgraph Control Plane Loop
        TrustCheck{Check TrustLevel}
        TrustCheck -->|OwnerCommand| RootAgent[Root Agent]
        TrustCheck -->|TrustedEvent| JobAgent[Job Agent]
        TrustCheck -->|UntrustedEvent| Sanitizer[Sanitizer Agent]
    end

    RootAgent -->|Full Access| Tools
    JobAgent -->|Scoped Access| Tools
    Sanitizer -->|No Tools| Artifacts

    subgraph Tools
        Native[Native Tools]
        Script[External Script Tools]
        Sandbox[Apple Sandbox / Isolation]
    end

    Tools -->|Result| AgentResponse
    AgentResponse --> Artifacts[Artifact Storage]

```

### Components

* **Ingress Bus:** A central `tokio` broadcast channel that normalizes all inputs into `IngressMessage` structs containing source, content, and `TrustLevel`.
* **Control Plane:** The main event loop. It routes `OwnerCommand` to the fully capable Root Agent and `UntrustedEvent` to a restricted Sanitizer persona that can only summarize text, not execute tools.
* **Artifact Store:** The primary memory system. Instead of unstructured chat logs, Zier-Alpha writes structured Markdown files (`Artifacts`) with strict provenance metadata (timestamp, model used, trust level).
* **Sandboxed Tools:** External scripts (Python/Node) are executed via the native OS sandbox (e.g., `sandbox-exec` on macOS) with strict profiles deny-listing network or file access by default.

## Workspace Structure

Zier-Alpha uses plain text files for state, making it Git-friendly and easy to edit manually.

```text
~/.zier-alpha/workspace/
├── MEMORY.md            # Long-term knowledge (auto-loaded context)
├── HEARTBEAT.md         # Autonomous task queue and status
├── SOUL.md              # System prompt and personality definition
└── artifacts/           # Structured outputs (reports, summaries)
    ├── 2024-03-20_finance_report.md
    └── 2024-03-21_hn_digest.md

```

## Configuration

Configuration is stored at `~/.zier-alpha/config.toml`.

```toml
[agent]
default_model = "claude-3-opus"
fast_model = "claude-3-haiku"

[ingress]
telegram_owner_id = 123456789

[sandbox]
default_policy = "strict" # Deny network, allow write only to artifacts/

[heartbeat]
enabled = true
interval = "30m"

[memory]
workspace = "~/.zier-alpha/workspace"

```

## License

Apache-2.0
