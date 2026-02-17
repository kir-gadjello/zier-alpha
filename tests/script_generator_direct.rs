use std::path::PathBuf;
use tempfile::TempDir;
use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_evaluate_generator_direct() {
    let temp_dir = TempDir::new().unwrap();
    let workspace = temp_dir.path().to_path_buf();

    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![workspace.to_str().unwrap().to_string()],
        allow_write: vec![workspace.to_str().unwrap().to_string()],
        allow_env: false,
        enable_os_sandbox: false,
    };

    let service = ScriptService::new(
        policy,
        workspace.clone(),
        workspace.clone(),
        WorkdirStrategy::Overlay,
        None,
        None,
        None,
        "test-agent".to_string(),
    )
    .expect("Failed to create script service");

    // Write generator script
    let script_content = r#"
        globalThis.generateSystemPrompt = (ctx) => {
            return `PROMPT: model=${ctx.model}`;
        };
    "#;
    let script_path = workspace.join("gen.js");
    std::fs::write(&script_path, script_content).unwrap();

    // Build context
    let context = serde_json::json!({
        "model": "test-model",
        "tool_names": ["bash"],
        "workspace_dir": workspace.to_str().unwrap(),
        "project_dir": null,
        "hostname": null,
        "current_time": "2026-02-17 07:00:00",
        "timezone": "UTC",
        "skills_prompt": null,
        "status_lines": null,
    });

    // Evaluate generator
    let result = service
        .evaluate_generator(&script_path, context)
        .await
        .expect("Generator evaluation failed");

    assert!(result.contains("PROMPT: model=test-model"));
}
