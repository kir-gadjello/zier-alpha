
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