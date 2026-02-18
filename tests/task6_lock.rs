use anyhow::Result;
use fs2::FileExt;
use std::fs::{self, File};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use zier_alpha::concurrency::WorkspaceLock;

#[test] // Sync test for simplicity with threads
fn test_workspace_lock_basics() -> Result<()> {
    // Setup
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();
    let lock_path = root.join("workspace.lock");
    let pid_path = root.join("workspace.lock.pid");

    // 1. Basic Acquisition
    let lock = WorkspaceLock::at_path(lock_path.clone())?;
    {
        let _guard = lock.acquire()?;
        assert!(lock_path.exists());
        assert!(pid_path.exists());
        let content = fs::read_to_string(&pid_path)?;
        assert_eq!(content, std::process::id().to_string());
        // guard dropped here
    }
    // Lock released. PID file might be gone (best effort).
    assert!(!pid_path.exists());

    // 2. Contention (Try Acquire)
    // Re-acquire in main thread manually to simulate external process
    let file = File::create(&lock_path)?;
    file.lock_exclusive()?;

    // Try acquire from another lock instance
    let lock2 = WorkspaceLock::at_path(lock_path.clone())?;
    let result = lock2.try_acquire()?;
    assert!(result.is_none(), "Should fail to acquire held lock");

    // Unlock
    file.unlock()?;
    drop(file);

    // Now try acquire should succeed
    let result = lock2.try_acquire()?;
    assert!(result.is_some(), "Should acquire free lock");

    Ok(())
}

#[tokio::test]
async fn test_workspace_lock_async_contention() -> Result<()> {
    // Setup
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();
    let lock_path = root.join("workspace.lock");

    let lock = WorkspaceLock::at_path(lock_path.clone())?;
    let lock_clone = lock.clone();

    // Barrier to synchronize lock acquisition
    let barrier = Arc::new(Barrier::new(2));
    let barrier_clone = barrier.clone();

    // Spawn a blocking task that holds the lock
    let handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let _guard = lock.acquire()?;
        // Signal that we have the lock
        barrier_clone.wait();
        // Hold it for a bit
        thread::sleep(Duration::from_millis(500));
        Ok(())
    });

    // Wait for the thread to acquire lock
    // This blocks the async task but Barrier::wait blocks thread.
    // In async test, we are on a thread.
    // If we block here, we block the runtime worker. That's fine for tests.
    barrier.wait();

    // Now we know lock is held. Try to acquire.
    let start = std::time::Instant::now();
    let guard = tokio::task::spawn_blocking(move || lock_clone.acquire()).await??;
    let elapsed = start.elapsed();

    assert!(elapsed >= Duration::from_millis(400), "Should have waited for lock release (elapsed: {:?})", elapsed);

    handle.await??;
    drop(guard);

    Ok(())
}
