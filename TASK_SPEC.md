# Task Specification: Extension Loading & Environment Inheritance Fixes

## Executive Summary

This specification addresses critical defects in the Hive and MCP extension integration that prevent tools from being properly exposed to the LLM and cause child process failures. The core issues are:

1. **Hive extension loaded after session creation** → System prompt does not include `hive_delegate` tool
2. **Environment inheritance gaps** → Child processes (`zier ask --child`) and MCP servers lose parent environment variables
3. **Insufficient test coverage** → No assertions verifying tool visibility in system prompt

These defects manifest as the LLM "not seeing" available tools, even when configured correctly.

---

## Problem Statement

### Observed Symptoms
- Hive delegation fails in release builds due to async race conditions (partially fixed)
- LLM does not call `hive_delegate` even when Hive is enabled in config
- MCP servers fail to start when their commands rely on `PATH` lookup (e.g., `npx`)
- Child processes spawned via Hive delegation cannot locate the `zier` binary or config
- No tests verify that enabled extensions actually make their tools available to the agent

### Root Causes
1. **Initialization Order Violation**: Hive extension is loaded *after* `agent.new_session()`. The system prompt is generated at session creation time and never updates when tools are added later.
2. **Environment Isolation**: `zier.os.exec` (Deno op) and MCP server spawning replace the environment instead of merging with the parent's, breaking commands that depend on `PATH`, `HOME`, `LD_LIBRARY_PATH`, etc.
3. **Missing Validation**: No integration tests assert that tool schemas and system prompts correctly reflect configured extensions.

---

## Strategic Objectives

### Primary Goals
- **G1**: Ensure all extension-provided tools are visible in the agent's system prompt before the first LLM call.
- **G2**: Guarantee child processes and MCP servers inherit the parent environment by default, with optional overrides.
- **G3**: Establish test coverage that verifies tool availability and system prompt content for enabled extensions.
- **G4**: Maintain backward compatibility with existing configs and workflows.

### Non-Goals
- Refactor extension architecture (out of scope for this fix)
- Implement new extension types
- Change MCP protocol implementation
- Modify existing LLM provider integrations (beyond ensuring they receive correct tool schemas)

---

## Technical Decisions

### Decision 1: Extension Loading Order
**Approach**: Load all extension-based tools *before* `agent.new_session()` is called.

**Rationale**:
- The system prompt is built in `Agent::new_session()` using `tool_executor.tools()`.
- Once the session is created, the system message is fixed; adding tools later does not update it.
- This requires reordering code in CLI commands (`ask`, `chat`) and potentially any daemon code that creates agents.

**Impact**:
- Affects `src/cli/ask.rs` and `src/cli/chat.rs`
- May affect `src/cli/daemon.rs` if agents are created there (verify)
- No breaking changes to public APIs

### Decision 2: Environment Inheritance for `zier.os.exec`
**Approach**: In `op_zier_exec`, merge `opts.env` with `std::env::vars()` instead of using `opts.env` exclusively.

**Implementation**:
```rust
let mut merged_env: HashMap<String, String> = std::env::vars().collect();
if let Some(ref overrides) = opts.env {
    merged_env.extend(overrides.clone());
}
// Use merged_env for Command::envs() or run_sandboxed_command()
```

**Rationale**:
- Child processes should inherit the parent environment by default (Unix/Windows convention).
- The `env` option should be additive/overriding, not replacement.
- Preserves `PATH`, `HOME`, `LD_LIBRARY_PATH`, `SHELL`, and other critical variables.
- Does not affect sandboxing policy, which still governs what commands are allowed.

**Security Consideration**: The `SafetyPolicy` already validates commands; environment merging does not bypass that. The sandbox still restricts capabilities via `SandboxState.capabilities`.

### Decision 3: Environment Inheritance for MCP Servers
**Approach**: In `McpManager::ensure_server`, call `cmd.envs(std::env::vars())` before applying server-specific `config.env`.

**Implementation**:
```rust
let mut cmd = Command::new(&config.command);
cmd.args(&config.args);
cmd.stdin(Stdio::piped());
cmd.stdout(Stdio::piped());
cmd.stderr(Stdio::piped());

// Inherit full parent environment first
cmd.envs(std::env::vars());
// Then apply server-specific overrides
if let Some(env) = config.env {
    cmd.envs(env);
}
```

**Rationale**:
- MCP servers are external processes that may rely on `PATH` to find Node.js/npx or other binaries.
- Without parent environment, servers fail to start on systems where binaries are not in default path.
- Overrides still allow customizing specific variables (e.g., `GITHUB_TOKEN`).

### Decision 4: Test Strategy
**New Integration Test**: `tests/context_visibility.rs`

**Test Coverage**:
1. **Hive Tool Registration**:
   - Create agent with `[extensions.hive] enabled = true`
   - Assert `hive_delegate` appears in `agent.tools()`
   - Assert system prompt includes `hive_delegate`

2. **MCP Tool Registration** (mock server):
   - Configure a mock MCP server that responds to `initialize` and `tools/list`
   - Assert MCP tools appear in `agent.tools()` and system prompt

3. **Session Creation Order**:
   - Verify that creating a session *after* extension loading captures all tools
   - Verify that adding tools after session creation does NOT update system prompt (document as undefined behavior)

**Rationale**:
- Integration tests catch the full end-to-end flow: config → agent → tools → system prompt.
- High-power assertions verify invariants that prevent regressions.

---

## Success Metrics

- ✅ `cargo test --release` passes, including `test_hive_integration` and new `test_context_visibility`
- ✅ Hive delegation works: parent LLM calls `hive_delegate`, child runs, response returns to parent
- ✅ Child processes can execute `zier` binary (inherits `PATH`)
- ✅ MCP servers start successfully when using `npx` or commands in `PATH`
- ✅ System prompt lists `hive_delegate` and any MCP tools when extensions enabled
- ✅ No breaking changes to existing configuration files

---

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Environment merging accidentally grants access to dangerous variables (e.g., `LD_PRELOAD`) | Low | Medium | `SafetyPolicy` already validates commands; env vars themselves are not executed. Document that extensions run with full parent env. |
| MCP server environment pollution | Low | Low | Server-specific overrides still work; inheritance is additive. |
| Breaking existing workflows that rely on empty child environment | Low | Medium | This would be a behavior change, but the current behavior is broken (child cannot find `zier`). If needed, add a opt-out flag (e.g., `--no-inherit-env`), but not in scope. |
| Test flakiness due to async timing | Medium | Medium | Use deterministic mocks where possible; await all async ops (already fixed). |

---

## Implementation Phases

### Phase 1: Critical Fixes (Day 1)
- Fix Hive loading order in `ask.rs` and `chat.rs`
- Fix `op_zier_exec` environment merging
- Fix MCP server environment inheritance
- Quick verification with `test_hive_integration`

### Phase 2: Testing & Validation (Day 2)
- Write `test_context_visibility.rs`
- Run full test suite in release mode
- Add unit tests for environment merging logic if needed

### Phase 3: Documentation & Cleanup (Day 3)
- Update any developer docs about extension loading
- Add comments to code explaining environment merging rationale
- Ensure `TODO.md` is complete

---

## References

- Audit Report: Provided in conversation context
- Key Files:
  - `src/cli/ask.rs`
  - `src/cli/chat.rs`
  - `src/scripting/deno.rs` (op_zier_exec)
  - `src/agent/mcp_manager.rs` (ensure_server)
  - `src/agent/mod.rs` (Agent::new_with_project, set_tools, new_session)
  - `tests/hive_integration.rs`
- Configuration: `config.example.toml` (extensions.hive, extensions.mcp)

---

## Appendix: Expected Behavior After Fix

### Scenario: Hive Delegation
1. User runs: `zier ask "delegate to echo: hello"`
2. Config has `[extensions.hive] enabled = true`
3. Agent initialization:
   - Builtins loaded (bash, read_file, etc.)
   - Hive extension script loaded from `extensions/hive/main.js`
   - `hive_delegate` tool registered via `ScriptTool`
   - Session created → system prompt includes `hive_delegate`
4. User query arrives, LLM (MockProvider) sees `hive_delegate` in tool schemas, returns tool call
5. Agent executes `hive_delegate` → orchestrator spawns child `zier ask --child ...`
6. Child process inherits parent `PATH` and `HOME`, finds binary, loads config, executes successfully
7. Child response returned to parent, printed to stdout

### Scenario: MCP Server
1. Config has MCP server with `command = "npx"` and `args = ["mcp-server-filesystem", "/allowed"]`
2. `Agent::new_with_project` calls `mcp_manager.ensure_server("server-name")`
3. Command built with full parent environment (including `PATH`)
4. `npx` resolves and spawns the MCP server
5. Tools discovered and added to agent's toolset
6. System prompt includes MCP tools
