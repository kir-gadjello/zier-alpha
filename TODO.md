# Implementation Plan: Extension Loading & Environment Inheritance

**Priority 1 (Critical)**: Fixes that break functionality in release builds
**Priority 2 (Important)**: Testing and validation
**Priority 3 (Polish)**: Documentation and edge cases

---

## Priority 1: Critical Fixes (Blocking)

### Task 1.1: Reorder Hive Extension Loading in `zier ask`
**File**: `src/cli/ask.rs`
**Goal**: Load Hive extension *before* `agent.new_session()`

**Steps**:
1. Cut the entire Hive loading block (lines where `if let Some(ref hive_config)` begins)
2. Paste it immediately after `let mut agent = Agent::new_with_project(...).await?;`
3. Keep `agent.new_session().await?;` as the next line after Hive loading completes
4. Ensure no other code is between Hive loading and `new_session()`

**Success Criteria**:
- Hive tools are present in `agent.tools()` when `new_session()` builds the system prompt
- System prompt includes `hive_delegate` when Hive enabled
- `cargo test --release` passes `test_hive_integration`

**Dependencies**: None

✅ **Completed**: 2025‑02‑14 – Hive loading moved before `new_session()`.

---

### Task 1.2: Reorder Hive Extension Loading in `zier chat`
**File**: `src/cli/chat.rs`
**Goal**: Same as Task 1.1, but for interactive chat

**Steps**:
1. Locate Hive loading block (around line 100+ in chat.rs)
2. Move it to immediately after `let mut agent = Agent::new_with_project(...).await?;`
3. Ensure session creation/resumption happens AFTER Hive tools are added
4. The code currently loads Hive after session creation/resumption logic — this must change

**Success Criteria**:
- Chat sessions include Hive tools in system prompt
- `test_hive_integration` still passes (uses `zier ask`, but validates consistency)

**Dependencies**: Task 1.1 (for pattern reference)

✅ **Completed**: 2025‑02‑14 – Hive loading moved before session creation/resumption.

---

### Task 1.3: Merge Environment in `op_zier_exec`
**File**: `src/scripting/deno.rs` (function `op_zier_exec`)
**Goal**: Child processes inherit parent environment, with optional overrides

**Steps**:
1. Before the `let output = if { ... }` block, add:
   ```rust
   let mut merged_env: HashMap<String, String> = std::env::vars().collect();
   if let Some(ref overrides) = opts.env {
       merged_env.extend(overrides.clone());
   }
   ```
2. In the sandboxed path, change:
   `run_sandboxed_command(&cmd[0], &args, &target_cwd, opts.env)`
   to
   `run_sandboxed_command(&cmd[0], &args, &target_cwd, Some(merged_env))`
3. In the direct path, replace:
   ```rust
   if let Some(env) = opts.env {
       command.envs(env);
   }
   ```
   with
   ```rust
   command.envs(&merged_env);
   ```
4. Add a comment explaining that environment merging is intentional for subprocess functionality

**Success Criteria**:
- Child processes (e.g., Hive-spawned `zier ask --child`) can find the binary via `PATH`
- They inherit `HOME` and other critical vars
- Existing sandbox policy checks remain intact
- No regression in `test_hive_integration`

**Dependencies**: None

✅ **Completed**: 2025‑02‑14 – Environment merging implemented.

---

### Task 1.4: Inherit Environment in MCP Server Spawning
**File**: `src/agent/mcp_manager.rs` (method `ensure_server`)
**Goal**: MCP servers inherit parent environment so `npx` and similar commands work

**Steps**:
1. After constructing `cmd` and before applying `config.env`, add:
   ```rust
   // Inherit parent environment to ensure PATH, HOME, etc. are available
   cmd.envs(std::env::vars());
   ```
2. Then apply server-specific overrides as before:
   ```rust
   if let Some(env) = config.env {
       cmd.envs(env);
   }
   ```
3. Add a comment explaining the inheritance rationale

**Success Criteria**:
- MCP servers using `command = "npx"` start successfully
- Logs show "Spawning MCP server: ..." without connection errors
- Tools from MCP servers appear in agent's tool list

**Dependencies**: None

✅ **Completed**: 2025‑02‑14 – MCP servers now inherit parent env.

---

## Priority 2: Testing & Validation

### Task 2.1: Write Integration Test `test_context_visibility`
**File**: `tests/context_visibility.rs` (new)
**Goal**: Assert that enabled extensions make tools visible in system prompt and tool list

**Implementation Details**:
- Use the pattern from `tests/hive_integration.rs` but focus on tool availability
- Copy Hive extension into temp dir (as done in hive_integration)
- Build `Config` with `[extensions.hive] enabled = true`
- Create agent, load Hive, create session
- Assert `hive_delegate` in `agent.tools()` (check `tools().iter().any(|t| t.name() == "hive_delegate")`)
- Assert system prompt content includes "hive_delegate"

**Optional**: Add MCP test with a mock MCP server that returns a fixed tool list. This requires more setup; if too complex, skip and rely on manual verification via `zier ask`.

**Success Criteria**:
- Test passes in release mode
- Test is deterministic and self-contained (no network calls)

**Dependencies**: Tasks 1.1-1.4 (must have fixes in place)

✅ **Completed**: 2025‑02‑14 – Test implemented and passing. Added `system_prompt()` getter to `Agent`.

---

### Task 2.2: Add Unit Test for Environment Merging
**File**: `src/scripting/deno.rs` (mod tests)
**Goal**: Verify `op_zier_exec` merges env correctly

**Approach**:
- Test that a mock command receives both inherited vars and overrides
- Use a simple script that prints env, capture output
- Alternatively, test the Rust helper logic by extracting env merge into a separate function (if refactored)

**Simpler**: Add integration test that runs `zier ask "test"` with a custom env var and verifies child sees it (via a script tool that returns `std::env::var("CUSTOM_VAR")`).

**Dependencies**: Task 1.3

---

### Task 2.3: Add Unit Test for MCP Environment Inheritance
**File**: `src/agent/mcp_manager.rs` (mod tests)
**Goal**: Verify `ensure_server` calls `cmd.envs(std::env::vars())`

**Approach**:
- Mock `Command` to capture env calls
- Use a fake command that exits immediately (e.g., `true` or `echo`)
- Assert that the spawned process has at least `PATH` from parent

**Simpler**: Manual test with `RUST_LOG=debug` and a server command that prints env to stderr.

**Dependencies**: Task 1.4

---

### Task 2.4: Run Full Test Suite in Release Mode
**Command**: `cargo test --release -- --nocapture`
**Goal**: Ensure all tests pass, especially:
- `test_hive_integration`
- `test_context_visibility` (new)
- `test_deno_tools` (sanity check for sandbox)

**Success Criteria**:
- 0 failures
- No flaky timing issues

**Dependencies**: All Priority 1 and 2.1-2.3 tasks

---

## Priority 3: Polish & Documentation

### Task 3.1: Update `config.example.toml`
**File**: `config.example.toml`
**Goal**: Add clear examples for Hive and MCP extensions

**Steps**:
- Uncomment or add `[extensions.hive]` section with `enabled = true`
- Add `[extensions.mcp]` with sample server configuration
- Document environment inheritance behavior in comments

**Dependencies**: None (can be done anytime)

---

### Task 3.2: Add Inline Comments to Critical Code
**Files**:
- `src/scripting/deno.rs` (around env merge)
- `src/agent/mcp_manager.rs` (around env inheritance)
- `src/cli/ask.rs` and `src/cli/chat.rs` (explaining why Hive loads before session)

**Goal**: Future maintainers understand the "why" behind these crucial lines

**Success Criteria**:
- Comments explain the initialization order dependency
- Comments explain environment inheritance necessity

**Dependencies**: Priority 1 tasks

---

### Task 3.3: Verify No Regression in Other Extensions
**Manual QA**:
- Test that scheduler, ingress, heartbeat still function
- Ensure changes to environment inheritance don't break existing behavior
- Check that desktop and server modes still start

**Success Criteria**:
- No new warning logs
- Background tasks run as expected

**Dependencies**: Priority 1 tasks

---

### Task 3.4: Update AGENTS.md (if applicable)
**File**: `AGENTS.md` (or project docs)
**Goal**: Document extension loading order and environment behavior

**Content**:
- "Extensions must be loaded before creating a session to appear in the system prompt"
- "Child processes and MCP servers inherit the parent environment by default"
- "Use opts.env to override specific variables"

**Dependencies**: Priority 1 tasks

---

## Checklist Definitions

Each task should be marked `[ ]` initially and `[x]` upon completion. The final state is all Priority 1 and Priority 2 tasks complete, with Priority 3 optional for the immediate release.

---

## Notes

- **Testing Philosophy**: Prefer deterministic integration tests over unit tests for these flows, as they involve many components. Use mock LLM providers (MockProvider is already in place).
- **Backward Compatibility**: These changes should not break existing configs. If a user hasn't enabled Hive/MCP, behavior is unchanged.
- **Performance**: Environment merging adds O(N) where N is number of env vars (~50-100), negligible.
- **Security**: No relaxation of sandbox policy; commands still go through `SafetyPolicy`.

---

## Estimated Effort

| Task | Priority | Est. Time |
|------|----------|-----------|
| 1.1 | P1 | 30 min |
| 1.2 | P1 | 30 min |
| 1.3 | P1 | 45 min |
| 1.4 | P1 | 30 min |
| 2.1 | P2 | 2 hours |
| 2.2 | P2 | 1 hour |
| 2.3 | P2 | 1 hour |
| 2.4 | P2 | 30 min |
| 3.1 | P3 | 30 min |
| 3.2 | P3 | 30 min |
| 3.3 | P3 | 1 hour |
| 3.4 | P3 | 30 min |

**Total**: ~10 hours for full completion (P1+P2), plus ~3 hours polish (P3).

---

## Sign-off Criteria

- [ ] All P1 tasks implemented and reviewed
- [ ] New integration test passes in release mode
- [ ] Existing test suite green (`cargo test --release`)
- [ ] Manual smoke test: `RUST_LOG=info zier ask "test"` shows Hive loading and tool schemas
- [ ] No new clippy warnings or format issues
- [ ] TOML config examples updated
- [ ] Code comments added for non-obvious logic
