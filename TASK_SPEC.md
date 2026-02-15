# Hive Refactor & Enhancement – Task Specification

**Priority:** P1 (Critical) – Blocks improved multi-agent workflows  
**Target:** `main` branch  
**Status:** Ready for estimation & execution  
**Authors:** Staff Engineer (AI-assisted)  
**Date:** 2026‑02‑15

---

## Executive Summary

Hive is Zier Alpha’s subagent orchestration extension. It enables the main agent to delegate tasks to specialized child agents running in isolated processes. The current implementation has ambiguities (`hive_delegate` name) and lacks key features: true cloning with byte‑identical context, configurable clone depth limits, and rich execution metadata.

This specification overhaul’s Hive with:

1. **Clearer API** – rename `hive_delegate` → `hive_fork_subagent`.
2. **Cloning mode** – spawn a subagent that is an exact temporal clone of the parent (identical system prompt, toolset, optional extra instructions).
3. **Safety limits** – `max_clone_fork_depth` prevents recursive fork bombs.
4. **Customization** – `clone_sysprompt_followup`, `clone_userprompt_prefix`, `clone_disable_tools`.
5. **Metadata** – parent receives structured usage info from child executions.

All changes are **backward‑compatible** for existing non‑clone usage (tool rename requires config updates, but old code can be migrated).

---

## 1. Problem Statement & Current Limitations

| Issue | Impact |
|-------|--------|
| `hive_delegate` name is confusing – does not convey subagent forking/sandboxing. | Poor UX; unclear mental model. |
| No true “clone” mode – child always gets a fresh system prompt with a new timestamp. | Context caching cannot be shared across parent/child; unnecessary token overhead. |
| Unlimited recursive delegation depth (only `max_depth` for named agents). | Fork bomb risk – a clone could call `hive_delegate` to create another clone, rapidly exhausting resources. |
| Parent receives only raw child output; no structured metadata (model, tokens, latency). | Hard to instrument, monitor, or debug delegation performance. |
| No way to inject extra system instructions or user‑prompt prefixes for clones. | Inflexible for multi‑agent patterns that need subtle context shifts. |
| No tool restrictions specific to clones (e.g., forbid recursive delegation). | Potential security/containment gap. |

---

## 2. Strategic Objectives

- **G1**: Rename tool to clearly express subagent forking.
- **G2**: Implement `tools: "."` as a true byte‑identical clone (including system prompt timestamp) via session hydration.
- **G3**: Prevent fork bombs with `max_clone_fork_depth` (default 1).
- **G4**: Support custom instructions (`clone_sysprompt_followup`) and prompt prefixes (`clone_userprompt_prefix`).
- **G5**: Allow disabling specific tools for clones (`clone_disable_tools`).
- **G6**: Return rich metadata from child to parent.
- **G7**: Maintain immutability of system prompts for all children (no time updates if already hydrated).
- **G8**: Provide thorough integration tests asserting invariants.

---

## 3. Non‑Goals

- **Dynamic tool discovery** across agents (out of scope).
- **Cross‑process context cache sharing** (beyond hydration) – we assume providers treat each session independently.
- **Changing existing named‑agent behavior** – only clone mode gets special handling (immutable system prompts, depth tracking).
- **Supporting multiple system messages** – `clone_sysprompt_followup` is appended to the existing system prompt, not inserted as a separate message.
- **Migrating old agent definitions automatically** – users must rename `hive_delegate` in their `.md` files.

---

## 4. Technical Decisions

### 4.1. System Prompt Immutability

**Decision:** Child processes launched with `--child` and `--hydrate-from` **must** use the hydrated session’s system message verbatim. No code path should call `build_system_prompt` for a child. The existing hydration mechanism already satisfies this; we will document and enforce it.

**Rationale:** The system prompt is part of the conversation history. Rewriting it (e.g., with a fresh time) would alter the semantic identity of the session and break context cache keys.

### 4.2. Clone Detection

**Decision:** A clone is defined as a delegation where the requested `agent` name is **empty** (`""`) or the special marker `"clone"` (to be decided). The parent detects this in the `hive_fork_subagent` tool handler and applies the cloning logic (toolset = parent toolset, `context_mode = "fork"`, pass `ZIER_HIVE_CLONE_DEPTH`).

**Rationale:** Simplicity – no need for a separate tool. The existing tool can support both named and anonymous delegation based on argument.

### 4.3. Tool Filtering Order

**Decision:** Parent computes the child’s effective tool list as:

```js
let effectiveTools = parentTools.slice();
if (cloneMode) {
  // Remove any tools listed in hive.clone_disable_tools
  effectiveTools = effectiveTools.filter(t => !clone_disable_tools.includes(t));
  // Also, if parent requested a named agent, that agent’s own `tools:` field will be applied in the child.
  // For clones, we ignore the (absent) agent definition and use the filtered parent list.
}
```

**Rationale:** Gives a single source of truth (parent) for what the child may use; prevents the child from re‑enabling `hive_fork_subagent` if it’s in the disable list.

### 4.4. Follow‑up vs. Prefix

- `clone_sysprompt_followup`: appended to the **system prompt** (i.e., part of the system message).
- `clone_userprompt_prefix`: prepended to the **user’s task** string before it is sent to the child.

These operate at different stages and do not interfere.

### 4.5. Depth Tracking

**Decision:** Introduce `ZIER_HIVE_CLONE_DEPTH` environment variable. Root parent has `0`. Named‑agent children inherit the same depth (they don’t increment). Clone children increment by 1. The child’s Hive implementation checks this against `max_clone_fork_depth` before allowing further clones.

**Rationale:** Separates clone‑fork depth from general Hive recursion depth (`ZIER_HIVE_DEPTH`) to allow independent limits.

### 4.6. Metadata Schema

Child process (via `src/cli/ask.rs` in `--child` mode) will write an IPC file with:

```json
{
  "status": "success" | "error",
  "content": "string",
  "error": "string | null",
  "metadata": {
    "agent": "echo",
    "model": "stepfun-flash",
    "provider": "openrouter",
    "latency_ms": 1234,
    "usage": { "input_tokens": 100, "output_tokens": 200 }
  }
}
```

Parent tool handler will format a readable result including these fields.

---

## 5. Configuration Schema (`HiveExtensionConfig`)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `true` | Master switch. |
| `agents_dir` | `String` | `"agents"` | Directory containing agent `.md` files. |
| `max_depth` | `usize` | `3` | Max Hive recursion depth for named agents. |
| `ipc_mode` | `String` | `"artifact"` | IPC mechanism (unchanged). |
| `default_model` | `String` | `""` (inherit) | Default model for agents not specifying one. |
| `timeout_seconds` | `u64` | `300` | Child process timeout. |
| `cleanup_temp_files` | `bool` | `true` | Whether to delete IPC/hydration temp files. |
| `allow_clones` | `bool` | `true` | If `false`, `hive_fork_subagent` with empty agent name is rejected. |
| `max_clone_fork_depth` | `usize` | `1` | Maximum clone‑chain length (root = 0; child with depth ≥ limit cannot fork further). |
| `clone_sysprompt_followup` | `Option<String>` | `None` | Additional text to append to the child’s system prompt. |
| `clone_userprompt_prefix` | `Option<String>` | `None` | String to prepend to the child’s task prompt. |
| `clone_disable_tools` | `Vec<String>` | `[]` | Tool names that should be **unavailable** to clone children. |

---

## 6. Invariants to Maintain & Test

1. **System prompt identity for clones**: For `tools: "."` (clone mode), the child’s full system prompt (including timestamp) must be **byte‑identical** to the parent’s at the moment of delegation. (Hydration ensures this.)
2. **Clone depth limit**: Any attempt to create a clone when `ZIER_HIVE_CLONE_DEPTH >= max_clone_fork_depth` fails with a clear error.
3. **Tool filtering**: Clones cannot invoke any tool listed in `clone_disable_tools`; attempts return “tool not found” or “tool disabled” errors.
4. **Follow‑up inclusion**: If `clone_sysprompt_followup` is set, the child’s system prompt contains that exact string after the workspace section.
5. **User‑prompt prefix**: Child receives the task string with prefix applied (if configured).
6. **Metadata presence**: Parent’s `hive_fork_subagent` result includes structured `metadata` object with `model`, `provider`, `latency_ms`, `usage`.
7. **Named‑agent independence**: Named agents are unaffected by clone‑specific logic (they get a fresh system prompt, normal depth tracking via `ZIER_HIVE_DEPTH`).

---

## 7. High‑Level Scope & Deliverables

### Rust Side

- `src/config/mod.rs`: Extend `HiveExtensionConfig`; add defaults.
- `src/cli/ask.rs`:
  - When `args.child`, capture usage & latency for IPC metadata.
  - Write enriched IPC JSON.
- `src/agent/system_prompt.rs`: Append `clone_sysprompt_followup` if env var `ZIER_HIVE_SYSPROMPT_FOLLOWUP` present.
- `src/agent/tools/system_introspect.rs` (optional test aid): Add flag to output the system prompt.

### Hive Extension (Deno)

- `extensions/hive/lib/registry.js`:
  - Register `hive_fork_subagent`.
  - Implement clone/named delegation logic with config checks.
  - Compute effective tool list (filtering `clone_disable_tools` for clones).
  - Apply `clone_userprompt_prefix`.
  - Set child env: `ZIER_HIVE_CLONE_DEPTH`, `ZIER_HIVE_AGENT_NAME`.
  - Call `zier.os.exec` with proper args (`--child`, `--hydrate-from` for clones).
  - Format result with metadata.
- `extensions/hive/lib/ipc.js`: Ensure `metadata` is read/written.

### Tests

- `tests/hive_clone_invariant.rs` – byte‑identity of system prompt between parent and clone.
- `tests/hive_clone_depth.rs` – depth limit enforcement.
- `tests/hive_clone_disabled_tools.rs` – tool blocking.
- `tests/hive_userprompt_prefix.rs` – prefix applied.
- `tests/hive_sysprompt_followup.rs` – follow‑up text appears.
- Update existing tests (`hive_integration.rs`, `context_visibility.rs`, `hive_inheritance.rs`) to use `hive_fork_subagent`.

### Documentation

- `README.md`: New “Hive Forking” section.
- `config.example.toml`: Document all new `[extensions.hive]` fields.
- `CHANGELOG.md`: Note tool rename and feature additions.

---

## 8. Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Fork bomb despite depth limit | Default `max_clone_fork_depth = 1` prevents recursion; enforce in child before spawning. |
| Child session loses system prompt identity | Verify hydration is used for clones; add test that clones have identical system prompt. |
| Metadata increases IPC payload size | Keep only essential fields (model, provider, latency, token counts). |
| Config field name collisions | Use explicit snake_case; review YAML/TOML schema. |
| Breaking existing agents | Bump minor version; provide migration guide (rename tool in agent files). |

---

## 9. Success Criteria

- All new and existing tests pass (`cargo test --release`).
- `cargo clippy` and `cargo fmt` clean.
- A clone child’s system prompt matches parent’s **exactly** (verified by integration test).
- Parent receives structured metadata for every subagent call.
- Documentation covers all new configuration knobs.

---

**End of Task Specification.**

