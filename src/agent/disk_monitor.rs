use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use fs2;
use tracing::{info, warn};
use crate::config::DiskConfig;

#[derive(Clone)]
pub struct DiskMonitor {
    config: DiskConfig,
    degraded_mode: Arc<AtomicBool>,
}

impl DiskMonitor {
    pub fn new(config: DiskConfig) -> Self {
        let monitor = Self {
            config,
            degraded_mode: Arc::new(AtomicBool::new(false)),
        };

        monitor.start_monitoring();
        monitor
    }

    pub fn is_degraded(&self) -> bool {
        self.degraded_mode.load(Ordering::Relaxed)
    }

    fn start_monitoring(&self) {
        let monitor = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60)); // Default check every minute

            // Parse interval from config if possible
            if let Ok(duration) = parse_duration(&monitor.config.monitor_interval) {
                interval = tokio::time::interval(duration);
            }

            loop {
                interval.tick().await;
                monitor.check_disk_space();
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
            Ok(available_bytes) => {
                match fs2::total_space(&path) {
                    Ok(total_bytes) => {
                        let available_percent = (available_bytes as f64 / total_bytes as f64) * 100.0;
                        let threshold = self.config.min_free_percent as f64;

                        let currently_degraded = self.is_degraded();

                        if available_percent < threshold {
                            if !currently_degraded {
                                warn!("Disk space low ({:.1}% free). Entering degraded mode.", available_percent);
                                self.degraded_mode.store(true, Ordering::Relaxed);
                            }
                        } else {
                            if currently_degraded {
                                info!("Disk space recovered ({:.1}% free). Exiting degraded mode.", available_percent);
                                self.degraded_mode.store(false, Ordering::Relaxed);
                            }
                        }
                    }
                    Err(e) => warn!("Failed to get total disk space: {}", e),
                }
            }
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
