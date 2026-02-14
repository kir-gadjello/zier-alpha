use std::io::Write;
use tempfile::NamedTempFile;
use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::scripting::ScriptService;

#[tokio::test]
async fn test_deno_tool_registration_and_execution() {
    // 1. Setup Sandbox Policy
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec!["/tmp".to_string()],
        allow_write: vec!["/tmp".to_string()],
        allow_env: false,
        enable_os_sandbox: false,
    };

    // 2. Initialize Service
    let temp_dir = tempfile::tempdir().unwrap();
    let service = ScriptService::new(
        policy,
        temp_dir.path().to_path_buf(),
        temp_dir.path().to_path_buf(),
        WorkdirStrategy::Overlay,
        None,
        None,
    )
    .expect("Failed to create script service");

    // 3. Create a JS script that registers a tool
    let script_content = r#"
        pi.registerTool({
            name: "test_echo",
            description: "Echoes input",
            parameters: {
                type: "object",
                properties: {
                    input: { type: "string" }
                }
            },
            execute: async (toolCallId, params) => {
                console.log("Executing test_echo");
                return JSON.stringify({ echo: params.input });
            }
        });
    "#;

    let mut script_file = NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();
    let script_path = script_file.path().to_str().unwrap().to_string();

    // 4. Load script
    service
        .load_script(&script_path)
        .await
        .expect("Failed to load script");

    // 5. Verify registration
    let tools = service.get_tools().await.expect("Failed to get tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "test_echo");

    // 6. Execute tool
    let result = service
        .execute_tool("test_echo", r#"{"input": "hello"}"#)
        .await
        .expect("Failed to execute tool");

    // Result should be JSON string
    let json: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(json["echo"], "hello");
}

#[tokio::test]
async fn test_deno_sandbox_fs_allowed() {
    let temp_file = NamedTempFile::new().unwrap();
    let temp_path = temp_file.path().to_str().unwrap().to_string();

    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![temp_path.clone()], // Allow reading specific file
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
    )
    .unwrap();

    let script_content = format!(
        r#"
        pi.registerTool({{
            name: "test_read",
            description: "Read file",
            parameters: {{}},
            execute: async () => {{
                try {{
                    const content = pi.readFile("{}");
                    return content;
                }} catch (e) {{
                    return "ERROR: " + e.message;
                }}
            }}
        }});
    "#,
        temp_path
    );

    let mut script_file = NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();

    service
        .load_script(script_file.path().to_str().unwrap())
        .await
        .unwrap();

    // Write content to read
    std::fs::write(&temp_path, "secret content").unwrap();

    let result = service.execute_tool("test_read", "{}").await.unwrap();
    // Result is JSON string of the return value (which is string content)
    // Wait, execute_tool returns what the JS execute returns.
    // If JS returns a string "secret content", execute_tool returns it as JSON string?
    // In deno.rs: `let json = serde_v8::from_v8(scope, value)?; Ok(json.to_string())`
    // So if it returns a string, it will be "\"secret content\"".

    assert_eq!(result, "secret content");
}

#[tokio::test]
async fn test_deno_sandbox_fs_denied() {
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![], // No read allowed
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
    )
    .unwrap();

    let script_content = r#"
        pi.registerTool({
            name: "test_read_denied",
            description: "Read file",
            parameters: {},
            execute: async () => {
                try {
                    pi.readFile("/etc/passwd"); // Should fail
                    return "SUCCESS";
                } catch (e) {
                    return "ERROR";
                }
            }
        });
    "#;

    let mut script_file = NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();

    service
        .load_script(script_file.path().to_str().unwrap())
        .await
        .unwrap();

    let result = service
        .execute_tool("test_read_denied", "{}")
        .await
        .unwrap();
    assert_eq!(result, "ERROR");
}
