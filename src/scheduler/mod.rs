use crate::ingress::IngressBus;
use anyhow::Result;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

pub mod dispatcher;

#[derive(Debug, Deserialize, Clone)]
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
    pub jobs: Vec<JobConfig>,
}

impl Scheduler {
    pub async fn new(bus: Arc<IngressBus>) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;
        Ok(Self {
            scheduler,
            bus,
            jobs: Vec::new(),
        })
    }

    pub async fn load_jobs(&mut self, config_path: &Path) -> Result<()> {
        if !config_path.exists() {
            info!(
                "No scheduler config found at {}, skipping.",
                config_path.display()
            );
            return Ok(());
        }

        let content = std::fs::read_to_string(config_path)?;
        let config: SchedulerConfig = toml::from_str(&content)?;
        self.jobs = config
            .job
            .iter()
            .map(|j| JobConfig {
                name: j.name.clone(),
                schedule: j.schedule.clone(),
                prompt_ref: j.prompt_ref.clone(),
                tool_ref: j.tool_ref.clone(),
            })
            .collect();

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
            info!(
                "Scheduled job: {} ({})",
                job_config.name, job_config.schedule
            );
        }
        Ok(())
    }

    pub async fn register_dynamic_job(
        &self,
        name: String,
        schedule: String,
        script_path: String,
    ) -> Result<()> {
        let bus = self.bus.clone();
        let job_name = name.clone();
        let script = script_path.clone();

        let job = Job::new_async(schedule.as_str(), move |_uuid, _l| {
            let bus = bus.clone();
            let name = job_name.clone();
            let script = script.clone();
            Box::pin(async move {
                let payload = format!("EXECUTE_SCRIPT: {}", script);
                let msg = crate::ingress::IngressMessage::new(
                    format!("scheduler:{}", name),
                    payload,
                    crate::ingress::TrustLevel::TrustedEvent,
                );
                if let Err(e) = bus.push(msg).await {
                    error!("Failed to push dynamic job {}: {}", name, e);
                }
            })
        })?;

        self.scheduler.add(job).await?;
        info!(
            "Registered dynamic job: {} ({}) -> {}",
            name, schedule, script_path
        );
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.scheduler.start().await?;
        Ok(())
    }
}
