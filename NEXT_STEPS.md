# Next Steps: Immediate Action Items

## For the Developer Continuing This Work

1. **Read `TASK_SPEC.md`** for full context and design rationale.
2. **Read `TODO.md`** for the step-by-step implementation checklist.

## Quick Start

```bash
# 1. Ensure you're on main and up to date
git checkout main
git pull

# 2. Create a feature branch
git checkout -b refactor/async-deno-ops

# 3. Implement Phase 1 first (check_path refactor)
# Follow TODO.md Tasks 1.1 and 1.2

# 4. Compile and check
cargo check --release

# 5. Run unit tests to ensure no regression
cargo test --release --lib

# 6. Implement Phase 2 (convert ops to async)
# Complete all Task 2.x steps

# 7. Run full integration suite focusing on previously hanging tests
cargo test --test deno_tools --release
cargo test --test tmux_bridge_e2e --release
cargo test --test mcp_e2e --release

# 8. If any test still hangs, add debug logging and investigate
# See TASK_SPEC.md "Rollback Plan" and "Verification Steps"

# 9. Once all tests pass, create a PR with reference to TASK_SPEC.md
```

## Important Notes

- The core change is in **one file**: `src/scripting/deno.rs`
- The fix involves **removing `std::fs` blocking calls** and switching to `tokio::fs` in ops marked `#[op2(async)]`
- No changes to JavaScript code are required—it already uses `await`
- No changes to tests are required—they will automatically run faster

## Expected Outcome

All integration tests that previously timed out should now complete within 30-60 seconds. The build remains green.

## If Stuck

- Re-read the "Root Cause Analysis" in `TASK_SPEC.md`
- Consult the original error logs from the build
- Check that you've updated **all** required ops: `op_read_file`, `op_write_file`, `op_fs_mkdir`, `op_fs_remove`, `op_fs_read_dir`, `op_write_file_exclusive`
- Ensure `check_path` has **no** `.exists()` or `.canonicalize()` calls
- Use `git diff` to compare against the original version if unsure

---

**Good luck!** This is a high-impact refactor that unblocks the entire test suite and improves the runtime architecture.
