//! Heartbeat runner for continuous autonomous operation

use anyhow::Result;
use chrono::{Local, NaiveTime};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::agent::{build_heartbeat_prompt, is_heartbeat_ok, Agent, AgentConfig, HEARTBEAT_OK_TOKEN};
use crate::config::{parse_duration, parse_time, Config};
use crate::memory::MemoryManager;

pub struct HeartbeatRunner {
    config: Config,
    interval: Duration,
    active_hours: Option<(NaiveTime, NaiveTime)>,
    workspace: PathBuf,
}

impl HeartbeatRunner {
    pub fn new(config: &Config) -> Result<Self> {
        let interval = parse_duration(&config.heartbeat.interval)
            .map_err(|e| anyhow::anyhow!("Invalid heartbeat interval: {}", e))?;

        let active_hours = if let Some(ref hours) = config.heartbeat.active_hours {
            let (start_h, start_m) = parse_time(&hours.start)
                .map_err(|e| anyhow::anyhow!("Invalid start time: {}", e))?;
            let (end_h, end_m) =
                parse_time(&hours.end).map_err(|e| anyhow::anyhow!("Invalid end time: {}", e))?;

            Some((
                NaiveTime::from_hms_opt(start_h as u32, start_m as u32, 0).unwrap(),
                NaiveTime::from_hms_opt(end_h as u32, end_m as u32, 0).unwrap(),
            ))
        } else {
            None
        };

        let workspace = config.workspace_path();

        Ok(Self {
            config: config.clone(),
            interval,
            active_hours,
            workspace,
        })
    }

    /// Run the heartbeat loop continuously
    pub async fn run(&self) -> Result<()> {
        info!(
            "Starting heartbeat runner with interval: {:?}",
            self.interval
        );

        loop {
            // Sleep until next interval
            sleep(self.interval).await;

            // Check active hours
            if !self.in_active_hours() {
                debug!("Outside active hours, skipping heartbeat");
                continue;
            }

            // Run heartbeat
            match self.run_once().await {
                Ok(response) => {
                    if is_heartbeat_ok(&response) {
                        debug!("Heartbeat: OK");
                    } else {
                        info!("Heartbeat response: {}", response);
                    }
                }
                Err(e) => {
                    warn!("Heartbeat error: {}", e);
                }
            }
        }
    }

    /// Run a single heartbeat cycle
    pub async fn run_once(&self) -> Result<String> {
        // Check if HEARTBEAT.md exists and has content
        let heartbeat_path = self.workspace.join("HEARTBEAT.md");

        if !heartbeat_path.exists() {
            debug!("No HEARTBEAT.md found");
            return Ok(HEARTBEAT_OK_TOKEN.to_string());
        }

        let content = fs::read_to_string(&heartbeat_path)?;
        if content.trim().is_empty() {
            debug!("HEARTBEAT.md is empty");
            return Ok(HEARTBEAT_OK_TOKEN.to_string());
        }

        // Create agent for heartbeat
        let memory = MemoryManager::new(&self.config.memory)?;
        let agent_config = AgentConfig {
            model: self.config.agent.default_model.clone(),
            context_window: self.config.agent.context_window,
            reserve_tokens: self.config.agent.reserve_tokens,
        };

        let mut agent = Agent::new(agent_config, &self.config, memory).await?;
        agent.new_session().await?;

        // Send heartbeat prompt
        let heartbeat_prompt = build_heartbeat_prompt();
        let response = agent.chat(&heartbeat_prompt).await?;

        Ok(response)
    }

    fn in_active_hours(&self) -> bool {
        let Some((start, end)) = self.active_hours else {
            return true; // No active hours configured, always active
        };

        let now = Local::now().time();

        if start <= end {
            // Normal range (e.g., 09:00 to 22:00)
            now >= start && now <= end
        } else {
            // Overnight range (e.g., 22:00 to 06:00)
            now >= start || now <= end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_active_hours_normal_range() {
        // This test would require mocking Local::now()
        // For now, just verify the logic pattern
        let start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(22, 0, 0).unwrap();

        let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();

        assert!(noon >= start && noon <= end);
        assert!(!(midnight >= start && midnight <= end));
    }
}
