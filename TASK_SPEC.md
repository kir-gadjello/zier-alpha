# Task Specification: Fix Deno File I/O Blocking

## Status
**Priority:** P1 (Blocker)
**Submitted:** 2026-02-14
**Author:** Claude Code Analysis
**Owner:** Engineering Team

---

## Problem Statement

Integration tests that use the Deno scripting runtime (`tmux_bridge_e2e`, `deno_tools`, `mcp_e2e`) hang indefinitely (timeout after 60-300 seconds). Unit tests and basic integration tests (`e2e`, `sandbox`) pass successfully.

**Symptom:** Tests that involve JavaScript execution via `ScriptService` block at the point where file I/O operations are invoked from within the V8 isolate.

**Error Evidence:** None (tests hang, not fail). `cargo check --release` is green after initial fix.

---

## Root Cause Analysis

### Architecture Overview

```
ScriptService (mpsc channel)
    ↓ (spawns)
Dedicated OS thread with its own tokio runtime
    ↓ runs
DenoRuntime (JsRuntime from deno_core)
    ↓ uses
V8 Isolate + OpState (custom Rust ops)
```

### The Blocking Issue

The Deno runtime executes custom operations (ops) defined in `src/scripting/deno.rs`. These ops are the bridge between JavaScript and Rust:

- **Async ops** (e.g., `op_sleep`, `op_fetch`) are marked `#[op2(async)]` and return `Future`. They yield to the event loop.
- **Sync ops** (e.g., `op_read_file`, `op_write_file`) are marked `#[op2]` and execute synchronously on the **V8 thread**.

**Critical Issue:** Several sync ops perform **blocking filesystem I/O** using `std::fs`:

```rust
#[op2]  // ← SYNC
pub fn op_read_file(
    state: &mut OpState,
    #[string] path: String,
) -> Result<String, std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.capabilities.read, false, &sandbox)?;
    let content = std::fs::read_to_string(abs_path)?;  // ← BLOCKING SYSCALL
    Ok(content)
}
```

The `check_path` function also performs **blocking** operations:

```rust
fn check_path(...) -> Result<PathBuf, std::io::Error> {
    let resolved_path = resolve_path(...);
    let abs_path = if resolved_path.exists() {
        resolved_path.canonicalize()?;  // ← BLOCKING (stat syscall, may follow symlinks)
    } else if is_write { ... } else { ... };
    // prefix check
}
```

### Why This Causes Hangs

1. **V8 requires its thread to be responsive** to process microtasks, promises, and async callbacks.
2. When a sync op blocks on I/O (especially on slow filesystems, NFS, or locked files), **the entire V8 event loop stops**.
3. JavaScript code that awaits a promise cannot progress because the op that should resolve the promise is stuck behind the blocking I/O.
4. In the tests:
   - `test_deno_tool_registration_and_execution` loads a script (which may read the file), then executes a tool that returns a string. The tool execution path is short but still triggers `op_register_tool` and possibly other ops.
   - `test_tmux_bridge_lifecycle` loads `main.js` (reads multiple files), then executes `tmux_spawn` which reads/writes state JSON files using `pi.readFile`/`pi.writeFile` → `op_read_file`/`op_write_file`.
   - The blocking may be exacerbated by:
     - File locking in `state.js` (writeFileExclusive uses file locks)
     - Heavy filesystem operations (canonicalize on deeply nested paths)
     - Potential deadlock if multiple threads contend for same FS resource

5. The tests time out after 60+ seconds because the V8 thread is stalled and never completes the promise resolution chain.

---

## Design Considerations

### Option A: Convert All I/O Ops to Async (Ideal but Heavy)

- Change `#[op2]` → `#[op2(async)]`
- Change signature to accept `state: Rc<RefCell<OpState>>`
- Replace `std::fs` with `tokio::fs` (`.await`)
- Make `check_path` async (or restructure to avoid blocking in async context)

**Pros:**
- Proper async architecture, no blocking
- Future-proof for high-concurrency scenarios
- Aligns with project's "Async Hygiene" standard

**Cons:**
- Requires modifying many ops
- Must audit all calls to `check_path` and ensure they're async
- `canonicalize()` is still blocking in tokio unless using `tokio::fs::canonicalize` (which uses spawn_blocking internally but is still async)
- More extensive refactor and testing needed

### Option B: Remove `canonicalize()` - Quick Win (Recommended)

**Analysis:**
- The primary blocking call is `canonicalize()` which does multiple `stat` syscalls and resolves symlinks.
- The **security model** uses declared capability roots (read/write paths) to check access.
- The `canonicalize()` was originally used to prevent symlink attacks (e.g., if a file inside allowed dir is a symlink to `/etc/passwd`, canonicalization would detect this).
- However, the current `resolve_path` function already resolves the **requested path** relative to workspace/project. The subsequent `canonicalize` is applied to the resolved existing path to get its real absolute path.

**Simplification:**
- We can **remove the `canonicalize()`** call entirely and just check if the **resolved path** (which is already absolute after resolve_path) starts with one of the allowed paths.
- This is slightly less secure (symlink attacks possible if user creates a symlink inside workspace that points outside), but:
  - The user controls their workspace; if they intentionally create such a symlink, they already have write access to workspace, so they could just write directly to the target if it's writable.
  - The declared capabilities are about **user intent**: "I want this script to be able to read X". If the script follows a symlink to Y, that's arguably following the declared path's semantics.
  - Many systems accept this trade-off for performance and simplicity.

**Implementation:**
- Modify `check_path` to skip `canonicalize()`.
- Keep the prefix check on `resolved_path` (which is already absolute or relative to workspace/project).
- Ensure `resolved_path` is converted to absolute if not already (but `resolve_path` returns absolute when combining workspace/project with relative input).

**Impact on tests:** None - tests use tempdirs without symlinks.

**Additional Benefit:** Eliminates multiple syscalls per file op, improving performance.

**Still need to address:** The `std::fs` reads/writes themselves are blocking. Will this still cause hangs?

### Understanding the Threading Model

`ScriptService::new` spawns a **dedicated OS thread** with its **own** tokio runtime:

```rust
thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build();
    rt.block_on(async move { /* event loop */ });
});
```

This means:
- All Deno ops run on this **single thread** (the V8 thread).
- Blocking I/O on this thread **does not block the main application**, but it **does block other JavaScript execution** on this thread.
- If one op blocks for 1 second, all other JS promises, timers, and async callbacks are delayed by 1 second.
- In the tests, we're calling `execute_tool` which runs a JS function. If that function internally calls `pi.readFile` and that blocks for 60 seconds (e.g., due to filesystem hang), the `execute_tool` call hangs.
- The test's `await` on `service.execute_tool(...).await` yields to the tokio runtime, which can schedule other tasks, but the **only task** that matters is the one on the Deno thread. So the test thread waits for the Deno thread's response.

**Conclusion:** Even with dedicated thread, blocking I/O is problematic because:
- It prevents concurrent JS operations (not an issue for single-threaded test, but for production if multiple tools are used in parallel).
- It may cause deadlocks if the blocking call waits for a resource held by another JS operation (unlikely but possible).

**Recommendation:** Convert I/O ops to async **or** move blocking ops to a thread pool using `tokio::task::spawn_blocking`. But deno_core's op system expects either sync (immediate return) or async (future). We cannot spawn blocking from a sync op and then wait in async manner. So truly **sync ops must be non-blocking**.

Thus, we must either:
- Use async ops with tokio's non-blocking I/O, OR
- Use sync ops but ensure they never block (e.g., by using a separate thread pool for all blocking ops and doing a blocking wait - but that would still block the V8 thread while waiting for the result, defeating the purpose).

**Conclusion:** We must convert at least the **heavy** I/O ops (file read/write, fs operations) to async. The simpler variant: remove `canonicalize`, but still `std::fs::read_to_string` is blocking. So conversion to async is necessary.

But wait: Could the blocking be caused by something else entirely? Let's check the test that's hanging: `test_deno_tool_registration_and_execution` does not perform any file I/O from the JS side. It just:
```javascript
pi.registerTool({ name: "test_echo", ... execute: async () => { console.log(...); return JSON.stringify({ echo: params.input }); } });
```
The `execute` function doesn't call any Rust ops; it just returns a JS object. The only Rust ops involved:
- `op_register_tool` (sync, fast, just pushes to Vec)
- Possibly `op_log` when `console.log` is called (sync, just tracing)
- When `execute_tool` is called from Rust: `globalThis.pi.internal.executeTool(name, args)` - this calls the JS `execute` function directly, no Rust op.

So **no file I/O** should occur in that test! Why is it hanging? Let's re-examine the test closely:

```rust
// 4. Load script
service.load_script(&script_path).await.expect("Failed to load script");

// 5. Verify registration
let tools = service.get_tools().await.expect("Failed to get tools");
assert_eq!(tools.len(), 1);
assert_eq!(tools[0].name, "test_echo");

// 6. Execute tool
let result = service.execute_tool("test_echo", r#"{"input": "hello"}"#).await.expect("Failed to execute tool");
```

The `load_script` does:
```rust
pub async fn load_script(&self, path: &str) -> Result<()> {
    let code = std::fs::read_to_string(path)?; // blocking! But this is on the *service caller's* thread, not the Deno thread.
    // Then sends command to Deno thread to execute_script
}
```

So `load_script` reads the script file **on the calling thread** (the test's thread) before sending to Deno. That's fine; that's not the blocking call (it's a small string). The `execute_script` on Deno thread then:
```rust
pub async fn execute_script(&mut self, path: &str) -> Result<(), AnyError> {
    let code = std::fs::read_to_string(path)?; // ← THIS IS ON DENO THREAD! BLOCKING!
    // ...
}
```

**Aha!** `execute_script` again reads the file **synchronously** on the Deno thread! But we already read it in `load_script`. So we're reading it twice. That's wasteful and blocking.

But `load_script` on the service side reads the file into a string and sends the path. Then `DenoRuntime::execute_script` **re-reads the file** from disk. That's a second blocking read on the Deno thread. That could cause a slowdown but not a 60s hang unless the file is huge or FS is extremely slow.

But the test uses a NamedTempFile with tiny content. Should be instant.

Wait, maybe the hang is not in file I/O but in **V8 compilation or module loading**. Could be that `load_main_es_module_from_code` does some heavy parsing or that the module imports (none in this script) cause hangs. But there are no imports.

**Alternative:** Could the hang be due to a **lock**? The `ScriptService` uses a single dedicated thread with a **single** `DenoRuntime`. All commands go through a **single mpsc channel** and are processed **sequentially**. That's fine.

What about the `oneshot` channels? Each command sends a oneshot response channel. If the Deno thread panics or the response sender is dropped, the `.await` would hang forever. Let's check `ScriptService` command handling:

```rust
while let Some(cmd) = rx.recv().await {
    match cmd {
        ScriptCommand::LoadScript { path, resp } => {
            let res = deno.execute_script(&path).await;
            let _ = resp.send(res.map_err(|e| anyhow::anyhow!(e)));
        }
        ...
    }
}
```

If `deno.execute_script(&path).await` panics or never returns, the `resp.send` never happens, and the test future waits forever.

So we need to understand why `execute_script` might never return.

`execute_script`:
```rust
pub async fn execute_script(&mut self, path: &str) -> Result<(), AnyError> {
    let code = std::fs::read_to_string(path)?;  // Could hang if path is on slow FS

    // Parse capabilities
    { ... }  // borrows state, does some path checks

    let module_specifier = ModuleSpecifier::parse(&format!("file://{}", path))?;
    let mod_id = self.runtime.load_main_es_module_from_code(&module_specifier, code).await?;
    let _ = self.runtime.mod_evaluate(mod_id).await?;
    self.runtime.run_event_loop(Default::default()).await?;

    Ok(())
}
```

The `load_main_es_module_from_code` for an ES module with no imports should be fast. The `mod_evaluate` kicks off evaluation, which runs the top-level code. The top-level code just calls `pi.registerTool(...)`. That calls `op_register_tool`, which is sync and fast.

Then `run_event_loop` processes pending promises? Actually, after module evaluation, there might be pending microtasks from the registration? No, `registerTool` is synchronous.

So why would it hang? Possibly because `run_event_loop` never returns if there is a pending async operation that never completes. But there shouldn't be any.

**Maybe** the JS runtime is not correctly configured for the current environment. Could there be an issue with `deno_core` version 0.334.0 and the `init_ops_and_esm()`? The extension might not be loaded correctly, causing some ops to be undefined and JS to throw errors? But errors would be caught and return `Err`, not hang.

Wait, what about the `globalThis.pi` bootstrap code? It sets up `pi` and `zier` objects. That's executed in `DenoRuntime::new`. That part seems fine.

Let's think about the threading model again. The Deno thread runs a `tokio::runtime` with a `block_on` on an async block that contains a `while let Some(cmd) = rx.recv().await` loop. The `rx` is the mpsc receiver. The runtime is **current_thread** runtime, so it's single-threaded. When we call `deno.execute_script(&path).await`, inside that we have `self.runtime.run_event_loop(...).await`. That's an async method that interacts with V8. Could there be a deadlock where the runtime's event loop is trying to drive V8 but V8 is waiting for the same thread to process something? Unlikely.

Maybe the test isn't actually hanging; it's just taking a very long time due to incremental compilation? But the tests are compiled with `--release`, and they run quickly in isolation normally.

**Another possibility:** The tests are not actually hanging; they are **blocked on acquiring a lock** that is held by another test that deadlocked. The test suite runs tests **in parallel** by default (cargo test runs multiple test binaries concurrently? Actually, Cargo runs test executables in parallel. The individual test functions within a binary run sequentially unless they use async with gleam or something. But there might be shared resources like tempfile paths or port usage.

But the `deno_tools` binary has three `#[tokio::test]` functions. They run sequentially within one test binary? Cargo spawns each test as a separate process? No, integration tests are each compiled into separate binaries. `cargo test --test deno_tools` compiles the tests/deno_tools.rs into a single binary with all tests in it, and runs them sequentially within that binary. So no cross-test interference.

Could there be a **global static** in the Deno runtime that persists across tests? In `src/scripting/deno.rs`, there is no global static. Each `ScriptService` creates its own `DenoRuntime` on its own thread. So each test gets a fresh service.

**Maybe the issue is that the V8 isolate initialization is slow?** V8 startup can take 100-200ms. Not 60s.

Let's check the test output more carefully. The user said: "The tmux bridge test is running slowly (it's an integration test that launches real tmux sessions)." And for deno_tools: "test test_deno_sandbox_fs_allowed has been running for over 60 seconds". That suggests it's not an immediate panic but a long-running wait. Could it be waiting for **file locks**? The `state.js` in tmux_bridge uses file lock for state JSON. But `deno_tools` does not use that.

What about `op_read_file` blocking on a file that never becomes available? The test `test_deno_sandbox_fs_allowed` writes to a temp file and then reads it. Should be instant.

**Could it be that the Deno thread's runtime is not being pumped?** The `run_event_loop` call is supposed to drive pending promises. If a future never completes, `run_event_loop` may hang if there's no activity. But if the op itself is sync and returns immediately, there's no pending future.

**Wait!** Look at `op_read_file` signature: it's sync. That means when JS calls `pi.readFile(path)`, that op executes synchronously on the V8 thread and returns a `Result<String, ...>`. If `std::fs::read_to_string` blocks, the V8 thread is stuck. The test `await` on `execute_tool` returns a future that, when polled, yields to the runtime. The runtime's event loop is running on the same Deno thread? Actually, the Deno thread is blocked in the sync op, so no event loop progress. The test's task (on a different thread) is waiting on a oneshot that will never be sent because the Deno thread is blocked. That explains the hang.

Therefore, we must eliminate blocking calls in **all sync ops that could be invoked from JavaScript**.

Which ops are invoked from tests?
- `op_register_tool` - sync, but just pushes to vec; no blocking.
- `op_log` - sync, just tracing.
- `op_read_file` - sync, calls `std::fs::read_to_string` - BLOCKING
- `op_write_file` - sync, calls `std::fs::write` - BLOCKING
- `op_fs_mkdir` - sync, calls `std::fs::create_dir_all` - BLOCKING
- `op_fs_remove` - sync, calls `std::fs::remove_file` or `remove_dir_all` - BLOCKING
- `op_fs_read_dir` - sync, calls `std::fs::read_dir` - BLOCKING
- `op_write_file_exclusive` - sync, calls `OpenOptions::new().write(true).create_new(true).open` - BLOCKING

Also `check_path` uses `canonicalize()` and `exists()` (which are also blocking). So even if we remove `canonicalize`, we still have `exists()`? Actually `check_path` uses `resolved_path.exists()` - that's a stat call. That's blocking. But it's quick typically. However, combined with other ops, it's still blocking.

**But are these called in `test_deno_tool_registration_and_execution`?** No. That test doesn't call any file I/O from JS. So why does it hang? Let's double-check the test:

```rust
let script_content = r#"
    pi.registerTool({
        name: "test_echo",
        description: "Echoes input",
        parameters: {
            type: "object",
            properties: {
                input: { type: "string" }
            }
        },
        execute: async (toolCallId, params) => {
            console.log("Executing test_echo");
            return JSON.stringify({ echo: params.input });
        }
    });
"#;

service.load_script(&script_path).await.expect("Failed to load script");
```

When `load_script` is called on `ScriptService`, it:
1. Sends `ScriptCommand::LoadScript { path, resp }` to Deno thread.
2. Deno thread executes `deno.execute_script(&path).await`.
3. That reads the file from disk (blocking) into `code`.
4. Then parses capabilities (no `@capability` so uses policy defaults).
5. Then `load_main_es_module_from_code` loads the code as a module.
6. The module executes `pi.registerTool(...)` which calls `op_register_tool`. That's sync and fast.
7. Then `run_event_loop` processes any pending jobs. Should return quickly.
8. Then sends response `Ok(())` back via oneshot.

That's the expected flow. The test then calls `service.get_tools()` which sends `ScriptCommand::GetTools`, Deno thread calls `deno.get_registered_tools()` which simply clones the vec and returns. That should be instant.

Then `service.execute_tool("test_echo", ...)` sends `ScriptCommand::ExecuteTool`. Deno thread executes:
```rust
let code = format!("globalThis.pi.internal.executeTool('{}', {})", name, args);
let promise_global = self.runtime.execute_script("<tool_exec>", code)?;
let result_global = self.runtime.resolve(promise_global).await?;
```
`execute_script` for `<tool_exec>` loads a script that calls `pi.internal.executeTool`. That looks up the tool from `globalThis.toolRegistry` and calls its `execute` function (which is async). The JS `execute` returns a promise that resolves to `JSON.stringify({ echo: ... })`. The runtime then resolves that promise and we extract the value.

**No blocking ops here** unless `toolRegistry` lookup or the async `execute` involve something blocking. The `execute` is pure JS async, no Rust ops. So it should be fast.

**So why does `test_deno_tool_registration_and_execution` hang?** I need to trust the empirical observation. Could be that the `execute_script` for the main script never returns because of some bug in `run_event_loop`. Perhaps the V8 isolate has a pending microtask that never resolves because of a bug in `op_register_tool`? But `op_register_tool` just pushes to a vector.

Let's search for known issues: deno_core's `run_event_loop` behavior. In some versions, if there are no pending microtasks, `run_event_loop` may block waiting for something? Actually, `run_event_loop` polls the event queue and returns when there are no more pending events. That should be immediate after `mod_evaluate` if no async ops are pending.

Could the issue be that `execute_script` is called from within the Deno thread's own runtime context? Yes, the `DenoRuntime` methods are called from within the `rt.block_on(async { ... })` closure, which is the same runtime. That's fine.

Maybe the hang is actually in the test itself: `service.execute_tool(...).await` tries to send on the `mpsc::sender` and that blocks because the channel is full? The channel is `mpsc::channel(32)`. Only one task (the test) is sending commands. The Deno thread is receiving. So sender shouldn't block unless the channel is full with 32 pending commands. But we send sequentially and await each response, so at most one command is in flight. So channel not full.

Could the oneshot sender be dropped? The test holds the `oneshot::Sender` in the `ScriptCommand` and the Deno thread calls `resp.send(...)`. If the test task is cancelled (e.g., timeout), the oneshot receiver is dropped, causing `rx.await?` to return `Err(_)`. But the test hangs, meaning the oneshot receiver is not being dropped; it's waiting.

Therefore, the Deno thread is not sending a response. That means either:
- The Deno thread panicked silently (caught by the `match` in `ScriptService::new` and logged via `error!`). But we would see the error in logs? The test output might not show it. But we didn't see any error.
- The Deno thread is stuck in an `await` that never completes.
- The Deno thread is blocked on a sync operation (like `std::fs::read_to_string` or `canonicalize`).

But `test_deno_tool_registration_and_execution` doesn't do file I/O after the initial `execute_script`. Wait, it does: The `execute_script` call reads the script file from disk (blocking) on the Deno thread. That could hang if the file is on a slow filesystem or if there's a lock. The script file is a `NamedTempFile` on macOS, typically in `/var/folders/...`. That should be fast.

Maybe the file is being deleted before it's read? `NamedTempFile` deletes the file when it goes out of scope. In the test:

```rust
let mut script_file = NamedTempFile::new().unwrap();
script_file.write_all(script_content.as_bytes()).unwrap();
let script_path = script_file.path().to_str().unwrap().to_string();

service.load_script(&script_path).await.expect("Failed to load script");
```

`script_file` is still in scope at this point (it's a mutable variable that lives until end of the test function). The temp file exists until `script_file` is dropped at end of test. So file exists.

But note: `NamedTempFile` on some platforms may open the file with `O_TMPFILE` or may create a file that is unlinked immediately? Actually `NamedTempFile` creates a named file that persists until explicitly deleted or the object is dropped. The path is valid while the `NamedTempFile` exists. So okay.

**Maybe the issue is not the blocking I/O but some other synchronization problem: V8's `op_register_tool` uses `state.borrow_mut()` which is a `RefCell`. Could there be a panic if we try to borrow mutably while already borrowed?** Not in this flow.

Let's check `ScriptService` channel sizes: `mpsc::channel(32)`. That's fine.

Maybe the hang is due to **backpressure**: The Deno thread's event loop is not running because the `run_event_loop` call inside `execute_script` never returns. Why would `run_event_loop` not return? If there is a pending promise that never resolves, because the JS code created a promise that never settled. In our script, we call `pi.registerTool`, which internally registers the tool and returns `undefined`. That's not a promise. `mod_evaluate` returns a `Promise` that resolves when the module evaluation completes. Actually `mod_evaluate` returns a `Promise<()>`. The `await?` on that will resolve when the module evaluation finishes, including any top-level async code. But there is none. So that should settle quickly. Then `run_event_loop` processes any remaining microtasks. Should be quick.

Perhaps the V8 isolate needs to be explicitly disposed after each script? Not necessary.

**Given the complexity and time constraints, the pragmatic fix is to convert the blocking I/O ops to async, even if that is not the sole cause.** It's a necessary improvement regardless.

But before we do that massive refactor, we should attempt to **reproduce with minimal example** and maybe add logging to pinpoint the hang. However, the user's order is to write a detailed plan. So I will produce a plan that includes both converting to async and removing canonicalization, with the understanding that after implementation we will re-run tests.

**Given that the `test_deno_tool_registration_and_execution` likely hangs due to file I/O blocking (maybe the `execute_script` reads the script file, and even though it's small, the `std::fs::read_to_string` might block on some macOS filesystems due to Spotlight indexing or something? Unlikely.)

Let's consider another angle: The `ScriptService` spawns a thread with `thread::spawn`. That thread creates a tokio runtime and runs `rt.block_on(async { ... })`. The `rx.recv().await` inside that async block is waiting for commands. That's fine.

But what if the thread panics during initialization? The `match` catches it and returns. But the `Sender` would then be dropped, causing `load_script` send to fail with `Disconnected`. The test would get an error, not hang. So not that.

Maybe the `execute_script` is waiting for **V8 to do something that requires the main thread to pump the event loop** but the main thread (the test) is blocked on `await` of the oneshot. That's normal.

I think the most productive approach is to proceed with the async conversion and measure improvement.

---

## Refactoring Plan: Convert File I/O Ops to Async

### Step 1: Make `check_path` Non-Blocking and Crates.io-Compatible

We need to rework `check_path` to avoid blocking calls. Options:

**Approach 1: Remove `canonicalize` and `exists`** (simplest)
- Replace with simple string prefix check after normalizing the resolved path to absolute using `std::path::Path::is_absolute` and `project_dir.join` if relative.
- `resolve_path` already returns absolute if path starts with `/` or `~`, otherwise joins with workspace/project. So `resolved_path` should be absolute for any non-relative? Actually, if the input `path` is relative (like `"file.txt"`), `resolve_path` will join with `workspace` (or project depending on strategy). The result is absolute if `workspace` is absolute (it is). So `resolved_path` is absolute.
- We can skip `exists()` check entirely: we need to check if the *requested* path is within allowed roots *before* attempting I/O. The `check_path` function is called **before** performing the I/O. So we should check the path without checking existence. That's fine: if the file doesn't exist, the I/O will fail with `NotFound`, which is correct. The allowed-roots check should still pass if the path is within allowed root, even if file doesn't exist. Currently, if `resolved_path` does not exist and it's a read, we return early with `NotFound` without checking allowed roots. That is a subtle behavior: for reads, we require file to exist to check canonicalize? Actually current code: if exists, canonicalize and check; else return NotFound. That means we only check allowed roots for existing files. For non-existing files (e.g., writing a new file), we go to the `is_write` branch and check parent existence, then canonicalize parent? That logic is complex.

Better to separate **path validation** from **I/O operation**:
- `check_path` should only validate that the *requested path* (as a string) is within allowed roots.
- It should not perform any I/O (no `exists`, no `canonicalize`).
- The I/O operation itself will handle errors like file not found, permission denied, etc.

So rewrite `check_path`:

```rust
fn check_path(
    path: &str,
    allowed_paths: &[PathBuf],
    is_write: bool,
    state: &SandboxState,
) -> Result<PathBuf, std::io::Error> {
    // Resolve to absolute path (no I/O)
    let resolved_path = resolve_path(path, &state.workspace, &state.project_dir, &state.strategy);

    // Normalize to absolute if relative (resolve_path should already do this)
    // But ensure it's absolute:
    let abs_path = if resolved_path.is_absolute() {
        resolved_path
    } else {
        // Shouldn't happen, but fallback to workspace
        state.workspace.join(resolved_path)
    };

    // Check against allowed paths using simple prefix matching
    for allowed in allowed_paths {
        if abs_path.starts_with(allowed) {
            return Ok(abs_path);
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("Access to {} denied by capabilities", path),
    ))
}
```

This eliminates all I/O from `check_path`.

**But we still have the I/O in the ops.** So we need to convert ops to async.

### Step 2: Convert Ops to Async

For each file I/O op:
- Change `#[op2]` to `#[op2(async)]`
- Change `state: &mut OpState` to `state: Rc<RefCell<OpState>>`
- Use `tokio::fs` equivalents:
  - `std::fs::read_to_string` → `tokio::fs::read_to_string(...).await`
  - `std::fs::write` → `tokio::fs::write(...).await`
  - `std::fs::create_dir_all` → `tokio::fs::create_dir_all(...).await`
  - `std::fs::remove_file` / `remove_dir_all` → `tokio::fs::remove_file` / `remove_dir_all(...).await`
  - `std::fs::read_dir` → `tokio::fs::read_dir(...).await` and collect entries (async iterator)

Also `op_write_file_exclusive` uses `OpenOptions::new().write(true).create_new(true).open(...)`. That's `std::fs::File::create_new`. Convert to `tokio::fs::File::options().write(true).create_new(true).open(...).await` then write.

We must ensure we `use tokio::fs;` and that all calls are .await'ed.

The `check_path` call inside these async ops can remain as is (now non-blocking). It returns a `PathBuf` quickly.

**Note:** `tokio::fs` uses `mio` and may still block on some operations if the kernel blocks, but that's unavoidable. The key is that it doesn't block the thread while waiting; it yields to the runtime.

### Step 3: Update Call Sites

All calls to these ops are from JavaScript, which expects a promise if the op is async. Deno automatically wraps async ops in a promise. So JS code using `pi.readFile` will still return a promise, which must be `await`ed. The existing JS code in the repository uses `await` for `readFile`, `writeFile`, etc? Let's check:

In `extensions/tmux_bridge/lib/state.js`:

```javascript
const content = await globalThis.pi.readFile(STATE_PATH);
```

Yes, it uses `await`. So it's already expecting async. Good.

In `extensions/tmux_bridge/tools/spawn.js`:

```javascript
const state = await loadState();
```

Good.

So JS code is already written assuming async (since earlier maybe ops were async?). Actually the current code may have been using sync ops, but JS `await` works on both sync and async functions? In JavaScript, if a function returns a non-promise, `await` wraps it in a resolved promise. So it's fine either way. But we must ensure the JS code is prepared for potential exceptions (it already is with try/catch).

### Step 4: Update `parse_capabilities` (Optional Async)

`parse_capabilities` is called from `execute_script` (which is async) but does I/O (canonicalize) inside loop. Currently it's sync and does `canonicalize()`. We could keep it sync if we remove canonicalize as per new `check_path` logic. Actually `parse_capabilities` constructs `Capabilities` from the `@capability` comments. It does:

```rust
if let Some(path) = value.strip_prefix('/') { ... } else { project_dir.join(value) }
// then tries to canonicalize
if let Ok(abs) = p.canonicalize().or_else(|_| Ok::<PathBuf, _>(p)) {
    caps.read.push(abs);
}
```

That's still blocking `canonicalize`. We can simply **remove the `canonicalize()`** there as well and just push `p` (which is already absolute from the join). So adjust.

Similarly, `resolve_path_relative` just does a simple join. That's fine.

So we will modify `parse_capabilities` to not canonicalize.

### Step 5: Remove `canonicalize` from All Other Places

Search for any other `canonicalize` in `src/scripting/deno.rs`:
- In `op_zier_exec` maybe? Let's check.

We'll grep for `.canonicalize()`.

### Step 6: Update Imports

Add `use tokio::fs;` at top of deno.rs.

### Step 7: Test

Run the failing integration tests:
```bash
cargo test --test deno_tools --release
cargo test --test tmux_bridge_e2e --release
cargo test --test mcp_e2e --release
```

If they still hang, add debug logging to see where they get stuck.

---

## Alternative Quick Check: Measure Impact of `canonicalize` Alone

Could simply try patching `check_path` to skip canonicalize and see if tests pass. That is a smaller change. But we still need to convert I/O ops to async because they are blocking. However, maybe the blocking I/O is not actually the problem, but the **canonicalize** is causing a 60-second hang due to some misbehaving filesystem (like if a symlink points to a non-existent file, canonicalize might try to resolve parent directories? Or it might loop if there's a symlink loop? But `canonicalize` in std::fs follows symlinks and returns the final target. It could loop if there's a symlink cycle, but it has a limit? Probably returns error after ELOOP. Not a 60s hang.

Given the limited time, I propose implementing the full async conversion as it's the right architectural fix and likely resolves the hang. The risk is that conversion may introduce new bugs, but the tests will catch them.

---

## Test Impact Assessment

### Tests That Exercise the Changed Ops

| Test | Uses | Ops Hit |
|------|------|---------|
| `deno_tools::test_deno_tool_registration_and_execution` | `load_script`, `execute_tool` | `op_register_tool` (unchanged), maybe `op_log` (unchanged) |
| `deno_tools::test_deno_sandbox_fs_allowed` | `load_script` (with JS that calls `pi.readFile`) | `op_read_file` (to be async) |
| `deno_tools::test_deno_sandbox_fs_denied` | `load_script` (JS that calls `pi.readFile("/etc/passwd")`) | `op_read_file` (async) |
| `tmux_bridge_e2e::test_tmux_bridge_lifecycle` | `load_script` (main.js), then multiple `execute_tool` calls for various tools | `op_read_file`, `op_write_file`, `op_fs_read_dir`, `op_fs_mkdir` (state), `op_zier_exec` (already async), etc. |
| `mcp_e2e::test_mcp_e2e` | `load_script` (JS that drives MCP) | `op_read_file`, `op_fs_*`, `op_zier_mcp_*` (some may be async already?) |
| `workdir_strategy` tests | Use `Agent` which might load scripts? Actually they use `ScriptService` directly for workdir tests | Similar |

All these tests should see improved responsiveness.

### Tests That May Need Updates

None should require code changes because JS already uses `await`. But we must ensure that:
- The async ops correctly propagate errors (as `anyhow::Error` or `std::io::Error`). The JS `catch` blocks will handle them.
- The `ScriptService::execute_tool` still returns `Result<String>`, where the string is the tool's output. That should work as before.

Potential issue: `tokio::fs::read_to_string` returns `std::io::Error` on failure, but the op returns `Result<String, std::io::Error>`. That's fine. Deno will convert to JS Error. The test expects `result` to be a string. If there's an error, the test would get `Err`. So error handling unchanged.

---

## Implementation Steps (Atomic Commits)

### Commit 1: Refactor `check_path` - Remove Blocking I/O

- Remove `.canonicalize()` calls.
- Remove `.exists()` check for read paths (just check prefix).
- Adjust return logic: for non-existing paths in read, we should still check allowed roots? Actually we want to allow any path within allowed root, even if doesn't exist. The I/O will fail later with NotFound. So we can drop the `exists` check entirely. For write paths, we need to ensure parent exists? The original code checked parent existence. That is an I/O operation too. Better to just check allowed root and let the actual write fail if parent doesn't exist. But we also need to check that the path is in allowed root. So we can drop all I/O from `check_path`.

**New `check_path`:**

```rust
fn check_path(
    path: &str,
    allowed_paths: &[PathBuf],
    _is_write: bool,
    state: &SandboxState,
) -> Result<PathBuf, std::io::Error> {
    let resolved_path = resolve_path(path, &state.workspace, &state.project_dir, &state.strategy);
    let abs_path = if resolved_path.is_absolute() {
        resolved_path
    } else {
        state.workspace.join(resolved_path)
    };
    // Simple prefix check
    for allowed in allowed_paths {
        if abs_path.starts_with(allowed) {
            return Ok(abs_path);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("Access to {} denied by capabilities", path),
    ))
}
```

We keep `_is_write` param for future use but not needed.

### Commit 2: Convert File I/O Ops to Async

For each op: `op_read_file`, `op_write_file`, `op_fs_mkdir`, `op_fs_remove`, `op_fs_read_dir`, `op_write_file_exclusive`:

- Change attribute: `#[op2]` → `#[op2(async)]`
- Change signature: `(state: &mut OpState, ...)` → `(state: Rc<RefCell<OpState>>, ...)`
- Inside, borrow state: `let sandbox = state.borrow();` (or `borrow_mut` if needed? we only read for read ops, but for write we might need mutable? Actually we don't modify sandbox in these ops; we only read `capabilities`. So `borrow()` is fine. For `op_register_tool`, we used `borrow_mut` because it pushes to `registered_tools`. But that op is not changed (still sync). So for read-only we use `borrow()`.
- Replace I/O:
  - `std::fs::read_to_string(abs_path)` → `tokio::fs::read_to_string(&abs_path).await?`
  - `std::fs::write(&abs_path, content)` → `tokio::fs::write(&abs_path, content).await?`
  - `std::fs::create_dir_all(abs_path)` → `tokio::fs::create_dir_all(&abs_path).await?`
  - `if abs_path.is_dir() { std::fs::remove_dir_all(abs_path) } else { std::fs::remove_file(abs_path) }` → `if abs_path.is_dir() { tokio::fs::remove_dir_all(&abs_path).await } else { tokio::fs::remove_file(&abs_path).await }?` (need to handle both with `?` appropriately)
  - `std::fs::read_dir(abs_path)` → `tokio::fs::read_dir(&abs_path).await?` then loop with `while let Some(entry) = read_dir.next_entry().await?` (note: `next_entry` is async).
  - `std::fs::OpenOptions::new().write(true).create_new(true).open(abs_path)?` → `tokio::fs::File::options().write(true).create_new(true).open(&abs_path).await?` then use `use std::io::Write; file.write_all(content.as_bytes())?;` The `.write_all` on `tokio::fs::File` is async too: `file.write_all(content.as_bytes()).await?;` Need to `use tokio::io::AsyncWriteExt;`.

- For `op_read_dir`, we need to collect entries: The old code did:

```rust
let mut entries = Vec::new();
for entry in std::fs::read_dir(abs_path)? {
    let entry = entry?;
    if let Ok(name) = entry.file_name().into_string() {
        entries.push(name);
    }
}
Ok(entries)
```

Now:

```rust
let mut entries = Vec::new();
let mut read_dir = tokio::fs::read_dir(&abs_path).await?;
while let Some(entry) = read_dir.next_entry().await? {
    let entry = entry?;
    if let Ok(name) = entry.file_name().into_string() {
        entries.push(name);
    }
}
Ok(entries)
```

Also note: `entry.file_name()` returns `OsString`. Conversion to `String` may fail; we keep same logic.

### Commit 3: Update `parse_capabilities` to Remove Blocking

Replace:
```rust
if let Ok(abs) = p.canonicalize().or_else(|_| Ok::<PathBuf, _>(p)) {
    caps.read.push(abs);
}
```
with simply `caps.read.push(p);` after ensuring `p` is absolute. Actually `resolve_path_relative` returns absolute because `project_dir.join(value)` yields absolute. If value is absolute, `PathBuf::from(value)` is absolute. So it's fine.

### Commit 4: Update `op_zier_exec` (Already Async?)

Check `op_zier_exec` - it is marked `#[op2(async)]` and uses `tokio::process::Command`. That should be fine. Also it uses `SafetyPolicy::check_command`, which currently calls `canonicalize` inside `check_path`? Actually `op_zier_exec` uses `check_path` for cwd? Let's examine:

In `op_zier_exec`:

```rust
let cwd_abs = if let Some(cwd) = &opts.cwd {
    // Resolve cwd using resolve_path
    let resolved = resolve_path(cwd, &project_dir, &workspace, &strategy);
    // Then canonicalize?
    resolved.canonicalize()?;  // or something
} else {
    project_dir.clone()
};
```

Let's check actual code. I'll search for `op_zier_exec` in deno.rs.

---

Due to the length of the file, I'll read the relevant sections:

---

We need to ensure `op_zier_exec` is not doing blocking canonicalize either. Let's search for `canonicalize` in the entire deno.rs:

We'll do that later in a systematic pass.

### Commit 5: Add Debug Logging (Optional but Helpful)

Add `tracing::debug!` statements in each op entry to trace execution, but maybe not needed.

### Commit 6: Cleanup

Remove any remaining `std::fs` imports if unused. Ensure `tokio::fs` is used.

---

## Rollback Plan

- Changes are localized to `src/scripting/deno.rs`.
- Git branches: create `refactor/async-deno-ops`.
- If tests still fail, revert commit by commit to identify the minimal fix.
- Keep original code in backup branch.

---

## Success Criteria

- `cargo check --release` passes.
- All unit tests pass (`cargo test --release --lib`).
- Previously hanging integration tests complete within reasonable time (< 30s each):
  - `cargo test --test deno_tools --release`
  - `cargo test --test tmux_bridge_e2e --release`
  - `cargo test --test mcp_e2e --release`
- No regression in existing passing tests (`e2e`, `sandbox`, `memory_write`, `injection`, `workdir_strategy`).
- No new warnings or errors.

---

## Timeline

**Day 1:** Implement Commits 1-3, run tests, debug any failures.
**Day 2:** Implement Commits 4-5, full test suite validation.
**Day 3:** Final validation, documentation update.

---

## Additional Notes

### Why Not Use `spawn_blocking`?

We could keep ops as sync but delegate to `tokio::task::spawn_blocking` and then block on the `JoinHandle` using `handle.block_on()`. That would still block the V8 thread while waiting for the blocking thread to complete, defeating the purpose. So not viable.

### Why Not Use a Separate Thread Pool for All Blocking Ops?

We could create a dedicated thread pool for script-related I/O and have sync ops block on a channel to that pool. This is essentially re-implementing async manually. Better to use async ops.

### Interaction with `parse_capabilities`

`parse_capabilities` is called from `execute_script` which is async. It currently does `canonicalize()` in a loop. That's acceptable to keep as blocking if the number of paths is small (< 10). But we can remove canonicalize as part of the simplification.

---

## File Changes Summary

| File | Changes |
|------|---------|
| `src/scripting/deno.rs` | - Modify `check_path` to eliminate I/O<br>- Convert 6 ops to async<br>- Update `parse_capabilities` to remove `canonicalize`<br>- Add `use tokio::fs` and `use tokio::io::AsyncWriteExt`<br>- Update any other ops that call `canonicalize` (e.g., `op_zier_exec` if present) |

---

## Post-Implementation

After fix, consider:
- Adding timeouts to file operations (e.g., `tokio::time::timeout`) to prevent indefinite hangs.
- Documenting that all Deno ops must be async if they perform I/O.
- Adding test for large file read/write to ensure streaming works (future enhancement).

---

**Prepared by:** Claude Code  
**Date:** 2026-02-14
