# Hive Refactor & Enhancement – Implementation Status

**Priority:** P1 (Critical)  
**Target:** `main` branch  
**Last updated:** 2026‑02‑15  

---

## Legend

- ✅ Completed
- 🔄 In Progress
- ⏳ Not Started
- ⚠️ Blocked/Issue

---

## Phase 1: Configuration Foundation

### ✅ Task 1.1: Extend HiveExtensionConfig with new fields

**Status:** ✅ Completed  
**Files:** `src/config/mod.rs`  
**Details:** Added `allow_clones`, `max_clone_fork_depth`, `clone_sysprompt_followup`, `clone_userprompt_prefix`, `clone_disable_tools`. Added `default_max_clone_fork_depth()`.  
**Verified:** `cargo check` passes.

---

### ✅ Task 1.2: Update config.example.toml and defaults

**Status:** ✅ Completed  
**File:** `config.example.toml`  
**Details:** Added commented examples for all new Hive clone settings.

---

## Phase 2: Tool Rename

### ✅ Task 2.1: Rename tool in Hive extension

**Status:** ✅ Completed  
**Files:** `extensions/hive/lib/tool.js`, `extensions/hive/lib/registry.js`  
**Details:** Changed tool name from `hive_delegate` to `hive_fork_subagent`. Updated description.

---

### ✅ Task 2.2: Update all test files to use new tool name

**Status:** ✅ Completed  
**Files:** `tests/hive_integration.rs`, `tests/context_visibility.rs`, `tests/hive_inheritance.rs`  
**Details:** Replaced all `"hive_delegate"` strings with `"hive_fork_subagent"`.

---

## Phase 3: Clone Mode Implementation (Parent Side)

### ✅ Task 3.1: Implement clone detection and pre‑checks in tool handler

**Status:** ✅ Completed  
**File:** `extensions/hive/lib/tool.js`  
**Details:** Handler now checks `allow_clones` and `max_clone_fork_depth`, applies `clone_userprompt_prefix`, constructs `runAgent` call with empty `agentName` for clones.

---

### ✅ Task 3.2: Update orchestrator to handle clones and depth tracking

**Status:** ✅ Completed  
**File:** `extensions/hive/lib/orchestrator.js`  
**Details:**
- Supports empty `agentName` for clone mode.
- Sets `ZIER_HIVE_CLONE_DEPTH` in child env (incremented from parent or 0).
- Forces `fork` mode for clones (`hydration` to preserve system prompt).
- Filters `effectiveTools` with `clone_disable_tools` in clone mode.
- Computes `effectiveModel` from parent context for clones.
- Returns full IPC data (not just `content`) to parent.

---

### ✅ Task 3.3: Parent formats result with metadata

**Status:** ✅ Completed  
**File:** `extensions/hive/lib/tool.js`  
**Details:** After `runAgent` returns IPC data, appends a `<metadata>` block to the tool result string containing `agent`, `model`, `provider`, `latency_ms`, and token usage.

---

## Phase 4: Child Process Enrichment

### ✅ Task 4.1: Capture usage, latency, provider in child IPC

**Status:** ✅ Completed  
**File:** `src/cli/ask.rs`  
**Details:**
- Measured latency before/after `agent.chat()`.
- Captured `agent.usage()` and `agent.provider_name()`.
- When `args.child` is true, added `metadata` object to the IPC JSON result.
- Includes `ZIER_HIVE_AGENT_NAME` from environment (defaults "clone").

---

### ✅ Task 4.2: Ensure child respects hydration (no system prompt rebuild)

**Status:** ✅ Implicit (existing logic)  
**File:** `src/cli/ask.rs`  
**Details:** `--hydrate-from` is processed before `new_session()` and `hydrate_from_file` loads the parent session, preserving system prompt exactly. No changes needed; documented invariant.

---

## Phase 5: System‑Prompt Follow‑Up

### ✅ Task 5.1: Append follow‑up text if configured

**Status:** ✅ Completed  
**File:** `src/agent/system_prompt.rs`  
**Details:** Checked `ZIER_HIVE_SYSPROMPT_FOLLOWUP` env var; if set, inserted an “## Additional Instructions” section after workspace section before time. Hive sets this env var from `clone_sysprompt_followup` config.

---

## Phase 6: Depth Limit Enforcement

### ✅ Task 6.1: Parent sets `ZIER_HIVE_CLONE_DEPTH` in child env

**Status:** ✅ Completed  
**File:** `extensions/hive/lib/orchestrator.js`  
**Details:** Child env includes `ZIER_HIVE_CLONE_DEPTH` (incremented for clones; unchanged for named agents).

---

### ✅ Task 6.2: Child tool handler checks max depth before forking

**Status:** ✅ Completed  
**File:** `extensions/hive/lib/tool.js`  
**Details:** In `execute`, if `isClone`, reads `ZIER_HIVE_CLONE_DEPTH` and compares to `max_clone_fork_depth` from config; errors if limit exceeded.

---

## Phase 7: Integration Tests (New)

### 🔄 Task 7.1: Clone depth limit test

**Status:** ⏳ Not Started  
**File:** `tests/hive_clone_depth.rs`  
**Details:** Set `max_clone_fork_depth = 2`. Parent creates clone (depth 1), then that clone attempts clone (depth 2) – should succeed; third (depth 3) must fail. Need to simulate using Hive tool calls with empty `agent_name`.

---

### 🔄 Task 7.2: Disabled tools test

**Status:** ⏳ Not Started  
**File:** `tests/hive_clone_disabled_tools.rs`  
**Details:** Set `clone_disable_tools = ["bash"]`. Parent creates clone, instructs clone to run a bash command – expect error. Verify other tools still work.

---

### 🔄 Task 7.3: User‑prompt prefix test

**Status:** ⏳ Not Started  
**File:** `tests/hive_userprompt_prefix.rs`  
**Details:** Set `clone_userprompt_prefix = "[CLONE] "`. Parent creates clone with task `"list files"`. Verify child received `"[CLONE] list files"` (by examining child’s session file or response).

---

### ⏳ Task 7.4: System‑prompt follow‑up test

**Status:** ⏳ Not Started  
**File:** `tests/hive_sysprompt_followup.rs`  
**Details:** Set `clone_sysprompt_followup = "You are a clone."`. Parent creates clone. Verify child’s system prompt contains the follow‑up text (via `system_introspect --sysprompt` or session file).

---

### ✅ Task 7.5: System‑prompt byte‑identity invariant test

**Status:** ✅ Completed (pre‑existing)  
**File:** `tests/hive_exact_clone_invariant.rs`  
**Details:** Verifies that base system prompts (excluding time) match between parent and clone.

---

## Phase 8: Documentation & Metadata

### ⏳ Task 8.1: Update README.md with Hive fork section

**Status:** ⏳ Not Started  
**File:** `README.md`  
**Details:** Document `hive_fork_subagent`, clone mode, and all new config options.

---

### ⏳ Task 8.2: Document new config fields in config.example.toml

**Status:** ✅ Already done in 1.2; may need minor tweaks

---

### ⏳ Task 8.3: Update CHANGELOG.md

**Status:** ⏳ Not Started  
**File:** `CHANGELOG.md`  
**Details:** Add entries: tool rename, clone features, metadata propagation.

---

## Phase 9: Finalization

### ⏳ Task 9.1: Remove any leftover `hive_delegate` references

**Status:** 🔄 Partial – grep to verify  
**Command:** `rg "hive_delegate" --type rs --type js --type toml`  
**Success:** No remaining references in codebase (except CHANGELOG history).

---

### ⏳ Task 9.2: Run full test suite and clippy

**Status:** ⏳ Not Started  
**Commands:** `cargo test --release -- --test-threads=4`, `cargo clippy --all-targets --all-features`  
**Success:** All tests pass; no warnings.

---

### ⏳ Task 9.3: Commit and push

**Status:** ⏳ Not Started  
**Details:** Ensure atomic commits; push to origin.

---

## Summary

| Category | Completed | In Progress | Not Started |
|----------|-----------|-------------|-------------|
| Configuration | 2 | 0 | 0 |
| Tool Rename | 2 | 0 | 0 |
| Clone Implementation | 6 | 0 | 0 |
| System Prompt Follow‑up | 1 | 0 | 0 |
| Depth Enforcement | 2 | 0 | 0 |
| Integration Tests | 1 | 0 | 4 |
| Documentation | 0 | 0 | 3 |
| Finalization | 0 | 0 | 3 |

**Total tasks:** 24  
**Completed:** 13  
**Remaining:** 11

---

**Next immediate actions:** Write integration tests for clone depth, disabled tools, userprompt prefix, sysprompt follow‑up; then documentation and cleanup.
