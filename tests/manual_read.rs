use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::scripting::ScriptService;
use tempfile::NamedTempFile;
use std::io::Write;

#[tokio::test]
async fn manual_read_test() {
    let temp_file = NamedTempFile::new().unwrap();
    let temp_path = temp_file.path().to_str().unwrap().to_string();

    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![temp_path.clone()],
        allow_write: vec![],
        allow_env: false,
        enable_os_sandbox: false,
    };

    let temp_dir = tempfile::tempdir().unwrap();
    let service = ScriptService::new(
        policy,
        temp_dir.path().to_path_buf(),
        temp_dir.path().to_path_buf(),
        WorkdirStrategy::Overlay,
        None,
        None,
    ).unwrap();

    // Write content to file
    std::fs::write(&temp_path, "secret content").unwrap();

    let script_content = format!(r#"
        pi.registerTool({{
            name: "test_read",
            execute: async () => {{
                const content = await pi.readFile("{}");
                return content;
            }}
        }});
    "#, temp_path);

    let mut script_file = NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();

    service.load_script(script_file.path().to_str().unwrap()).await.unwrap();

    let result = service.execute_tool("test_read", "{}").await.unwrap();
    println!("Result: {}", result);
    assert_eq!(result, "secret content");
}
