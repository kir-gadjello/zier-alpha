use crate::ingress::{IngressBus, IngressMessage, TrustLevel};
use anyhow::Result;
use tracing::info;

pub async fn dispatch_job(bus: &IngressBus, name: &str, prompt_ref: &str) -> Result<()> {
    info!("Job triggered: {}", name);
    let payload = format!("EXECUTE_JOB: {}", prompt_ref);
    let msg = IngressMessage::new(
        format!("scheduler:{}", name),
        payload,
        TrustLevel::TrustedEvent,
    );
    bus.push(msg).await?;
    Ok(())
}
