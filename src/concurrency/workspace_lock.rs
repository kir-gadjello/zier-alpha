//! Cross-process workspace lock using advisory file locking (fs2 flock).
//!
//! Serializes all agent turns across processes (daemon, CLI, desktop)
//! so that shared workspace files (MEMORY.md, sessions.json, etc.)
//! are never written concurrently.

use anyhow::Result;
use fs2::FileExt;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

/// Advisory file lock for the agent workspace.
///
/// Lock file lives at `~/.zier-alpha/workspace.lock` (outside the workspace
/// to avoid git/watcher noise).
#[derive(Clone)]
pub struct WorkspaceLock {
    path: PathBuf,
    pid_path: PathBuf,
}

/// RAII guard that releases the lock on drop.
pub struct WorkspaceLockGuard {
    file: File,
    pid_path: PathBuf,
}

impl Drop for WorkspaceLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        // Best effort cleanup of PID file
        let _ = fs::remove_file(&self.pid_path);
    }
}

impl WorkspaceLock {
    /// Create a new WorkspaceLock.
    ///
    /// The lock file is placed at `~/.zier-alpha/workspace.lock`.
    pub fn new() -> Result<Self> {
        let state_dir = crate::agent::get_state_dir()?;
        let path = state_dir.join("workspace.lock");
        let pid_path = state_dir.join("workspace.lock.pid");
        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path, pid_path })
    }

    /// Create a new WorkspaceLock at a custom path.
    pub fn at_path(path: PathBuf) -> Result<Self> {
        let pid_path = path.with_extension("lock.pid");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(Self { path, pid_path })
    }

    /// Blocking acquire — waits until the lock is available.
    ///
    /// Returns an RAII guard that releases the lock on drop.
    pub fn acquire(&self) -> Result<WorkspaceLockGuard> {
        let start = Instant::now();
        let timeout = Duration::from_secs(30);

        loop {
            let file = File::create(&self.path)?;
            match file.try_lock_exclusive() {
                Ok(()) => {
                    // Write PID
                    let _ = fs::write(&self.pid_path, std::process::id().to_string());
                    return Ok(WorkspaceLockGuard {
                        file,
                        pid_path: self.pid_path.clone(),
                    });
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || (cfg!(unix)
                            && (e.raw_os_error() == Some(35) || e.raw_os_error() == Some(11))) =>
                {
                    if start.elapsed() > timeout {
                        // Check PID for debugging, but do not delete file (dangerous with fs2)
                        if let Ok(content) = fs::read_to_string(&self.pid_path) {
                            if let Ok(pid) = content.trim().parse::<u32>() {
                                if !self.is_process_running(pid) {
                                    tracing::warn!("Timeout acquiring lock. Owner PID {} appears dead, but lock is still held (OS issue?).", pid);
                                } else {
                                    tracing::warn!("Timeout acquiring lock. Held by running PID {}.", pid);
                                }
                            }
                        }
                        anyhow::bail!("Timed out acquiring workspace lock");
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    #[cfg(unix)]
    fn is_process_running(&self, pid: u32) -> bool {
        Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(true) // Assume running if check fails
    }

    #[cfg(not(unix))]
    fn is_process_running(&self, _pid: u32) -> bool {
        true
    }

    /// Non-blocking try-acquire — returns `None` if another process holds it.
    pub fn try_acquire(&self) -> Result<Option<WorkspaceLockGuard>> {
        let file = File::create(&self.path)?;
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = fs::write(&self.pid_path, std::process::id().to_string());
                Ok(Some(WorkspaceLockGuard {
                    file,
                    pid_path: self.pid_path.clone(),
                }))
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            #[cfg(unix)]
            Err(ref e) if e.raw_os_error() == Some(35) || e.raw_os_error() == Some(11) => {
                // EAGAIN(11) / EWOULDBLOCK(35 on macOS) — lock contention
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    /// Helper to create a WorkspaceLock pointing at a temp directory
    fn test_lock(dir: &std::path::Path) -> WorkspaceLock {
        WorkspaceLock {
            path: dir.join("test.lock"),
            pid_path: dir.join("test.lock.pid"),
        }
    }

    #[test]
    fn acquire_and_release() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = test_lock(tmp.path());

        let guard = lock.acquire().unwrap();
        // Lock is held — drop releases it
        drop(guard);

        // Can re-acquire after drop
        let _guard2 = lock.acquire().unwrap();
    }

    #[test]
    fn try_acquire_returns_none_when_held() {
        let tmp = tempfile::tempdir().unwrap();
        let lock_path = tmp.path().join("test.lock");

        // Hold the lock from a raw file
        let file = File::create(&lock_path).unwrap();
        file.lock_exclusive().unwrap();

        let lock = WorkspaceLock {
            path: lock_path.clone(),
        };
        let result = lock.try_acquire().unwrap();
        assert!(result.is_none(), "try_acquire should return None when held");

        // Release
        file.unlock().unwrap();
        drop(file);

        let result = lock.try_acquire().unwrap();
        assert!(result.is_some(), "try_acquire should succeed after release");
    }

    #[test]
    fn guard_drop_releases_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = test_lock(tmp.path());

        {
            let _guard = lock.acquire().unwrap();
            // Guard is alive
        }
        // Guard dropped, lock should be released

        // Another acquire should succeed immediately
        let _guard2 = lock.acquire().unwrap();
    }

    #[test]
    fn concurrent_threads_serialize() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().to_path_buf();
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let barrier = Arc::new(Barrier::new(3));

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let p = path.clone();
                let c = counter.clone();
                let b = barrier.clone();
                std::thread::spawn(move || {
                    let lock = test_lock(&p);
                    b.wait(); // all threads start together
                    let _guard = lock.acquire().unwrap();
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}
