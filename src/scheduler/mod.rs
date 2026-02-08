use tokio_cron_scheduler::{Job, JobScheduler};
use crate::ingress::IngressBus;
use serde::Deserialize;
use std::sync::Arc;
use anyhow::Result;
use tracing::{info, error};
use std::path::Path;

pub mod dispatcher;

#[derive(Debug, Deserialize)]
pub struct JobConfig {
    pub name: String,
    pub schedule: String,
    pub prompt_ref: String,
    pub tool_ref: String,
}

#[derive(Debug, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub job: Vec<JobConfig>,
}

pub struct Scheduler {
    scheduler: JobScheduler,
    bus: Arc<IngressBus>,
}

impl Scheduler {
    pub async fn new(bus: Arc<IngressBus>) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        Ok(Self { scheduler, bus })
    }

    pub async fn load_jobs(&mut self, config_path: &Path) -> Result<()> {
        if !config_path.exists() {
            info!("No scheduler config found at {}, skipping.", config_path.display());
            return Ok(());
        }

        let content = std::fs::read_to_string(config_path)?;
        let config: SchedulerConfig = toml::from_str(&content)?;

        for job_config in config.job {
            let bus = self.bus.clone();
            let name = job_config.name.clone();
            let prompt_ref = job_config.prompt_ref.clone();
            let schedule = job_config.schedule.clone();

            // Create job
            let job = Job::new_async(schedule.as_str(), move |_uuid, _l| {
                let bus = bus.clone();
                let name = name.clone();
                let prompt_ref = prompt_ref.clone();
                Box::pin(async move {
                    if let Err(e) = dispatcher::dispatch_job(&bus, &name, &prompt_ref).await {
                        error!("Failed to dispatch job {}: {}", name, e);
                    }
                })
            })?;

            self.scheduler.add(job).await?;
            info!("Scheduled job: {} ({})", job_config.name, job_config.schedule);
        }
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }
}
