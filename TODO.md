# TODO: Convert Deno File I/O Ops to Async

**Project:** Zier Alpha  
**Task:** Fix blocking file I/O in Deno ops causing test timeouts  
**Priority:** P1 (Blocker)  
**Owner:** Engineering Team  
**Created:** 2026-02-14  

---

## Overview

Integration tests (`deno_tools`, `tmux_bridge_e2e`, `mcp_e2e`) hang indefinitely due to synchronous blocking file I/O operations in the Deno runtime's custom ops. We need to convert these ops to async using tokio's async filesystem APIs and remove blocking `canonicalize()` calls.

---

## Implementation Checklist

### Phase 0: Preparation

- [ ] Create a feature branch: `refactor/async-deno-ops`
- [ ] Ensure `main` or `master` builds and passes unit tests
- [ ] Create backup of `src/scripting/deno.rs` (or rely on git)

### Phase 1: Eliminate Blocking in Path Validation

**File:** `src/scripting/deno.rs`

**Task 1.1:** Refactor `check_path` function
- [ ] Remove `.exists()` check - we should validate path against allowed roots regardless of existence
- [ ] Remove `.canonicalize()` call entirely
- [ ] Ensure `resolve_path` returns absolute path (it already does for workspace/project joins)
- [ ] Simplify to a pure path prefix check with no I/O:
  ```rust
  fn check_path(path: &str, allowed_paths: &[PathBuf], _is_write: bool, state: &SandboxState) -> Result<PathBuf, std::io::Error> {
      let resolved_path = resolve_path(path, &state.workspace, &state.project_dir, &state.strategy);
      let abs_path = if resolved_path.is_absolute() { resolved_path } else { state.workspace.join(resolved_path) };
      for allowed in allowed_paths {
          if abs_path.starts_with(allowed) { return Ok(abs_path); }
      }
      Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Access to {} denied", path)))
  }
  ```
- [ ] Remove unnecessary `is_write` parameter usage if any (keep for signature compatibility)
- [ ] Add comment explaining that path validation is now I/O-free

**Task 1.2:** Update `parse_capabilities` to remove `canonicalize`
- [ ] Locate the loop that processes `read` and `write` declarations
- [ ] Replace:
  ```rust
  if let Ok(abs) = p.canonicalize().or_else(|_| Ok::<PathBuf, _>(p)) {
      caps.read.push(abs);
  }
  ```
  with:
  ```rust
  caps.read.push(p);
  ```
  (since `p` is already absolute from `resolve_path_relative` or from absolute input)
- [ ] Ensure `resolve_path_relative` returns `PathBuf` that is absolute (it does for absolute paths and joins with `project_dir` for relative)
- [ ] Test that `parse_capabilities` still works with both absolute and relative paths

### Phase 2: Convert File I/O Ops to Async

**Task 2.1:** Add tokio imports
- [ ] At top of file add:
  ```rust
  use tokio::fs;
  use tokio::io::AsyncWriteExt;
  ```
  (remove unused `std::fs` if no longer needed, but still used elsewhere)

**Task 2.2:** Convert `op_read_file`
- [ ] Change attribute: `#[op2]` → `#[op2(async)]`
- [ ] Change signature: `state: &mut OpState` → `state: Rc<RefCell<OpState>>`
- [ ] Inside: borrow state with `let sandbox = state.borrow();` (read-only)
- [ ] Replace `std::fs::read_to_string(abs_path)?` with `tokio::fs::read_to_string(&abs_path).await?`
- [ ] Return `Result<String, std::io::Error>` (same error type)

**Task 2.3:** Convert `op_write_file`
- [ ] `#[op2(async)]`
- [ ] `state: Rc<RefCell<OpState>>`
- [ ] `tokio::fs::write(&abs_path, content).await?`

**Task 2.4:** Convert `op_fs_mkdir`
- [ ] `#[op2(async)]`
- [ ] `state: Rc<RefCell<OpState>>`
- [ ] `tokio::fs::create_dir_all(&abs_path).await?`

**Task 2.5:** Convert `op_fs_remove`
- [ ] `#[op2(async)]`
- [ ] `state: Rc<RefCell<OpState>>`
- [ ] Replace:
  ```rust
  if abs_path.is_dir() { std::fs::remove_dir_all(abs_path) } else { std::fs::remove_file(abs_path) }
  ```
  with:
  ```rust
  if abs_path.is_dir() { tokio::fs::remove_dir_all(&abs_path).await } else { tokio::fs::remove_file(&abs_path).await }?;
  ```
  (use `?` to propagate error from either branch; note both return `Result<(), std::io::Error>`)

**Task 2.6:** Convert `op_fs_read_dir`
- [ ] `#[op2(async)]`
- [ ] `state: Rc<RefCell<OpState>>`
- [ ] Replace:
  ```rust
  let mut entries = Vec::new();
  for entry in std::fs::read_dir(abs_path)? {
      let entry = entry?;
      if let Ok(name) = entry.file_name().into_string() { entries.push(name); }
  }
  Ok(entries)
  ```
  with:
  ```rust
  let mut entries = Vec::new();
  let mut read_dir = tokio::fs::read_dir(&abs_path).await?;
  while let Some(entry) = read_dir.next_entry().await? {
      let entry = entry?;
      if let Ok(name) = entry.file_name().into_string() { entries.push(name); }
  }
  Ok(entries)
  ```

**Task 2.7:** Convert `op_write_file_exclusive`
- [ ] `#[op2(async)]`
- [ ] `state: Rc<RefCell<OpState>>`
- [ ] Use tokio's File with options:
  ```rust
  let mut file = tokio::fs::File::options()
      .write(true)
      .create_new(true)
      .open(&abs_path)
      .await?;
  file.write_all(content.as_bytes()).await?;
  Ok(())
  ```
- [ ] Add `use tokio::io::AsyncWriteExt;` at top

**Task 2.8:** Verify other ops
- [ ] Check `op_zier_exec` - already async? Confirm: it uses `#[op2(async)]`. Does it call any blocking functions? It may call `check_path` (now non-blocking) and `SafetyPolicy::check_command` (which may call `canonicalize`? Need to check).
- [ ] Search for any remaining `std::fs` calls in the file that are inside op implementations. Replace with tokio async counterparts as needed.
- [ ] Search for any `canonicalize()` calls in the entire file and remove or replace with non-blocking alternative.

### Phase 3: Clean Up and Verify

**Task 3.1:** Ensure `use` statements
- [ ] Add `use tokio::fs;` and `use tokio::io::AsyncWriteExt;`
- [ ] Check if `std::fs` is still needed elsewhere (e.g., in `parse_capabilities`? We removed it there. Maybe in `resolve_path`? That's pure path manipulation, no fs. So we can remove `use std::fs;` if it was only for those ops. But careful: other parts of the file may still use `std::fs` (like reading a file somewhere else). Scan file for remaining `std::fs::` usage.

**Task 3.2:** Compile
- [ ] Run `cargo check --release` and fix any compilation errors
- [ ] Ensure no warnings about unused imports

**Task 3.3:** Run initial tests
- [ ] `cargo test --release --lib` (should pass)
- [ ] `cargo test --test e2e --release` (should pass)
- [ ] `cargo test --test sandbox --release` (should pass)
- [ ] `cargo test --test memory_write --release` (should pass)
- [ ] `cargo test --test injection --release` (should pass)

**Task 3.4:** Run Previously Hanging Tests
- [ ] `cargo test --test deno_tools --release` (should complete within 30s)
- [ ] `cargo test --test tmux_bridge_e2e --release` (should complete within 120s)
- [ ] `cargo test --test mcp_e2e --release` (should complete within 120s)
- [ ] `cargo test --test workdir_strategy --release` (should still pass)

**Task 3.5:** If any test still hangs
- [ ] Add debug logging to each op to see which one blocks
- [ ] Use `strace` or `dtruss` on macOS to see if any thread is stuck in kernel
- [ ] Check for other blocking calls (e.g., `resolve_path_relative` uses `canonicalize`? Let's verify)
- [ ] Search for any remaining `canonicalize()` calls in the crate, especially in `resolve_path` (in `src/agent/tools.rs` maybe). That function is used by `check_path` and might be called from other places. But we already removed canonicalize from check_path; however other ops might call `resolve_path` before `check_path`. `resolve_path` itself might call `canonicalize`? Let's check.

### Phase 4: Cross-Crate Impact

**Task 4.1:** Check `src/agent/tools.rs` for `resolve_path`
- [ ] Locate `resolve_path` function
- [ ] Determine if it does any I/O (should be pure path manipulation)
- [ ] If it does `canonicalize`, consider whether it's called from hot paths. It's not in Deno ops after our changes because we moved path resolution out.

**Task 4.2:** Ensure `SandboxState` capabilities are not mutated from async ops
- [ ] All our converted ops read `capabilities` via `state.borrow()`. That's fine.
- [ ] No mutable borrow required, so no risk of deadlock.

### Phase 5: Documentation and Regression Prevention

**Task 5.1:** Update coding standards doc (if any) to state: "All Deno ops that perform I/O must be async using tokio."
- [ ] Add example in CONTRIBUTING.md or similar

**Task 5.2:** Add test to ensure no sync blocking ops remain
- [ ] Write a lint or test that scans `src/scripting/deno.rs` for `#[op2]` without `async` that call `std::fs` or `std::path::Path::canonicalize`. (Maybe future enhancement)

**Task 5.3:** Commit with detailed message
- [ ] Include reference to this TODO/TASK_SPEC in commit

---

## Verification Steps (Before Closing)

For each test, verify:
- **No hangs:** Test completes within expected time (unit tests < 10s, integration < 60s)
- **No failures:** All assertions pass
- **No warnings:** `cargo test` output shows no deprecation warnings related to our changes

Example command:
```bash
# Full suite with timeout via timeout command (optional)
time cargo test --release -- --test-threads=1  # run sequentially to avoid interference
```

If any test still hangs:
- Run with `RUST_LOG=debug` to see logs
- Run with `RUST_BACKTRACE=1` to capture stack traces (may need to send SIGINT to see where it's stuck)
- Use `gdb` or `lldb` to attach and get backtrace of hung thread

---

## Rollback Steps

If changes cause regressions that cannot be quickly fixed:
```bash
git checkout main -- src/scripting/deno.rs
# or
git revert <commit_sha> -m 1
```

---

## Dependencies

- Tokio 1.43 (already in use)
- deno_core 0.334 (compatible with async ops)
- No external crates needed

---

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Async conversion introduces subtle race conditions | Medium | Thorough testing, ensure no shared mutable state without guards. All state access via `RefCell` is single-threaded (same thread) so safe. |
| `tokio::fs` behavior differs from `std::fs` (e.g., error ordering) | Low | Tests will catch differences. |
| Memory leak if `Rc<RefCell<OpState>>` not properly dropped | Low | The pattern is standard for deno_core async ops. |
| Some JS code expects sync return (unlikely) | Low | JS uses `await` so fine. |
| Performance regression due to async overhead | Low | I/O bound anyway; overhead negligible. |

---

## Success Definition

- All previously hanging tests pass within 2x their expected duration (typical I/O-bound runtime).
- No new flakiness introduced.
- Code review approval from at least one other engineer.

---

**End of TODO**
