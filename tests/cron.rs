use zier_alpha::ingress::{IngressBus, TrustLevel};
use zier_alpha::scheduler::dispatcher::dispatch_job;

#[tokio::test]
async fn test_cron_dispatch() {
    let bus = IngressBus::new(10);
    let job_name = "test_job";
    let prompt_ref = "scouts/test";

    // Dispatch
    dispatch_job(&bus, job_name, prompt_ref).await.unwrap();

    // Verify
    let receiver = bus.receiver();
    let mut rx = receiver.lock().await;

    if let Some(msg) = rx.recv().await {
        assert_eq!(msg.source, "scheduler:test_job");
        assert_eq!(msg.payload, "EXECUTE_JOB: scouts/test");
        assert_eq!(msg.trust, TrustLevel::TrustedEvent);
    } else {
        panic!("No message received on bus");
    }
}
