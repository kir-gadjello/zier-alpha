use crate::config::DiskConfig;
use fs2;
use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct DiskMonitor {
    config: DiskConfig,
    degraded_mode: Arc<AtomicBool>,
}

impl DiskMonitor {
    pub fn new(config: DiskConfig) -> Arc<Self> {
        // If disabled via environment (e.g., in tests or CI with disk pressure),
        // create a monitor that never degrades and does not spawn background task.
        if env::var("ZIER_ALPHA_DISABLE_DISK_MONITOR").is_ok() {
            return Arc::new(Self {
                config,
                degraded_mode: Arc::new(AtomicBool::new(false)),
            });
        }

        let monitor = Arc::new(Self {
            config,
            degraded_mode: Arc::new(AtomicBool::new(false)),
        });

        Self::start_monitoring(&monitor);
        monitor
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded_mode.load(Ordering::Relaxed)
    }

    pub async fn cleanup(&self) -> anyhow::Result<String> {
        let mut report = Vec::new();

        // 1. Cleanup Sessions
        if self.config.session_retention_days > 0 {
            if let Ok(state_dir) = crate::agent::get_state_dir() {
                // We iterate over all agents? Or just check typical paths.
                // Assuming standard layout: ~/.zier-alpha/agents/<agent>/sessions/
                // We can't easily enumerate all agents without SessionManager helper.
                // But we can check "main" and "http" agents at least.
                // Or scan `agents/` dir.

                let agents_dir = state_dir.join("agents");
                if agents_dir.exists() {
                    if let Ok(entries) = tokio::fs::read_dir(&agents_dir).await {
                        let mut entries = entries;
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                                let sessions_dir = entry.path().join("sessions");
                                match self
                                    .cleanup_directory(
                                        &sessions_dir,
                                        "jsonl",
                                        self.config.session_retention_days,
                                    )
                                    .await
                                {
                                    Ok(count) => {
                                        if count > 0 {
                                            report.push(format!(
                                                "Deleted {} old sessions for agent {:?}",
                                                count,
                                                entry.file_name()
                                            ));
                                        }
                                    }
                                    Err(e) => error!(
                                        "Failed to cleanup sessions for {:?}: {}",
                                        entry.file_name(),
                                        e
                                    ),
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Cleanup Logs
        if let Ok(state_dir) = crate::agent::get_state_dir() {
            let logs_dir = state_dir.join("logs");
            let max_mb = self.config.max_log_size_mb;

            if max_mb > 0 {
                // Calculate total size
                let mut total_size = 0;
                let mut log_files = Vec::new();

                if let Ok(mut entries) = tokio::fs::read_dir(&logs_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if entry
                            .path()
                            .extension()
                            .map(|e| e == "log")
                            .unwrap_or(false)
                        {
                            if let Ok(meta) = entry.metadata().await {
                                total_size += meta.len();
                                log_files.push((
                                    entry.path(),
                                    meta.modified().unwrap_or(SystemTime::now()),
                                ));
                            }
                        }
                    }
                }

                // Delete oldest if over size limit
                if total_size > (max_mb as u64 * 1024 * 1024) {
                    log_files.sort_by_key(|k| k.1); // Sort by modified time (oldest first)

                    let mut deleted_count = 0;
                    for (path, _) in log_files {
                        if total_size <= (max_mb as u64 * 1024 * 1024) {
                            break;
                        }
                        if let Ok(meta) = tokio::fs::metadata(&path).await {
                            let size = meta.len();
                            if let Err(e) = tokio::fs::remove_file(&path).await {
                                error!("Failed to delete log file {}: {}", path.display(), e);
                            } else {
                                total_size = total_size.saturating_sub(size);
                                deleted_count += 1;
                            }
                        }
                    }
                    if deleted_count > 0 {
                        report.push(format!(
                            "Deleted {} log files to enforce size limit",
                            deleted_count
                        ));
                    }
                }
            }
        }

        if report.is_empty() {
            Ok("Disk cleanup completed. No files eligible for deletion.".to_string())
        } else {
            Ok(report.join("\n"))
        }
    }

    async fn cleanup_directory(
        &self,
        dir: &Path,
        extension: &str,
        retention_days: u32,
    ) -> anyhow::Result<usize> {
        if !dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut entries = tokio::fs::read_dir(dir).await?;
        let now = SystemTime::now();
        let retention = Duration::from_secs(retention_days as u64 * 24 * 60 * 60);

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().map(|e| e == extension).unwrap_or(false) {
                if let Ok(metadata) = entry.metadata().await {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age > retention {
                                if let Err(e) = tokio::fs::remove_file(&path).await {
                                    error!("Failed to delete {}: {}", path.display(), e);
                                } else {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(count)
    }

    fn start_monitoring(monitor: &Arc<Self>) {
        let weak_monitor = Arc::downgrade(monitor);
        let interval_duration =
            parse_duration(&monitor.config.monitor_interval).unwrap_or(Duration::from_secs(60));

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                interval.tick().await;
                if let Some(monitor) = weak_monitor.upgrade() {
                    monitor.check_disk_space();
                } else {
                    break; // Monitor dropped
                }
            }
        });
    }

    fn check_disk_space(&self) {
        // Check space on the home directory (or where state is stored)
        let path = if let Some(base) = directories::BaseDirs::new() {
            base.home_dir().to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        };

        match fs2::available_space(&path) {
            Ok(available_bytes) => match fs2::total_space(&path) {
                Ok(total_bytes) => {
                    let available_percent = (available_bytes as f64 / total_bytes as f64) * 100.0;
                    let threshold = self.config.min_free_percent;

                    let currently_degraded = self.is_degraded();

                    if available_percent < threshold {
                        if !currently_degraded {
                            warn!(
                                "Disk space low ({:.1}% free). Entering degraded mode.",
                                available_percent
                            );
                            self.degraded_mode.store(true, Ordering::Relaxed);
                        }
                    } else {
                        if currently_degraded {
                            info!(
                                "Disk space recovered ({:.1}% free). Exiting degraded mode.",
                                available_percent
                            );
                            self.degraded_mode.store(false, Ordering::Relaxed);
                        }
                    }
                }
                Err(e) => warn!("Failed to get total disk space: {}", e),
            },
            Err(e) => warn!("Failed to check disk space: {}", e),
        }
    }
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    // Simple parser: "10m", "1h", "30s"
    let len = s.len();
    if len < 2 {
        return Err("Invalid duration format".to_string());
    }
    let (num_str, unit) = s.split_at(len - 1);
    let num: u64 = num_str.parse().map_err(|_| "Invalid number".to_string())?;

    match unit {
        "s" => Ok(Duration::from_secs(num)),
        "m" => Ok(Duration::from_secs(num * 60)),
        "h" => Ok(Duration::from_secs(num * 3600)),
        _ => Err("Unknown unit".to_string()),
    }
}
