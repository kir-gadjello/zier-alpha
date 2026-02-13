use zier_alpha::config::{SandboxPolicy, WorkdirStrategy};
use zier_alpha::scripting::ScriptService;
use tempfile::NamedTempFile;
use std::io::Write;
use std::path::PathBuf;

#[tokio::test(flavor = "current_thread")]
#[cfg_attr(target_os = "macos", ignore)]
async fn test_mcp_e2e() {
    let _ = tracing_subscriber::fmt::try_init();
    // 1. Setup Mock Server Path
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mock_server_path = manifest_dir.join("tests/fixtures/mock_mcp_server.py");

    // Ensure mock server is executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&mock_server_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&mock_server_path, perms).unwrap();
    }

    // 2. Setup Sandbox Policy
    let policy = SandboxPolicy {
        allow_network: false,
        allow_read: vec![mock_server_path.to_str().unwrap().to_string()],
        allow_write: vec![],
        allow_env: false,
        enable_os_sandbox: false,
    };

    // 3. Initialize Service
    let temp_dir = tempfile::tempdir().unwrap();
    let service = ScriptService::new(
        policy,
        temp_dir.path().to_path_buf(),
        temp_dir.path().to_path_buf(),
        WorkdirStrategy::Overlay,
        None,
        None
    ).expect("Failed to create script service");

    // 4. Create JS script to drive MCP
    // We construct the config object in JS directly for the test
    let script_content = format!(r#"
    try {{
        const config = {{
            servers: [
                {{
                    name: "mock",
                    command: "python3",
                    args: ["-u", "{}"],
                    env: {{}}
                }}
            ]
        }};

        console.log("JS: Starting script");
        await zier.mcp.initialize(config.servers);

        console.log("JS: Calling ensureServer");
        await zier.mcp.ensureServer("mock");
        console.log("JS: ensureServer returned");

        pi.registerTool({{
            name: "test_list",
            description: "List tools",
            parameters: {{ type: "object", properties: {{}} }},
            execute: async () => {{
                try {{
                    const tools = await zier.mcp.listTools("mock");
                    return JSON.stringify(tools);
                }} catch (e) {{
                    return "ERROR: " + e.message;
                }}
            }}
        }});

        pi.registerTool({{
            name: "test_call_echo",
            description: "Call echo",
            parameters: {{ type: "object", properties: {{ text: {{ type: "string" }} }} }},
            execute: async (args) => {{
                try {{
                    const res = await zier.mcp.call("mock", "echo", args);
                    return JSON.stringify(res);
                }} catch (e) {{
                    return "ERROR: " + e.message;
                }}
            }}
        }});

        pi.registerTool({{
            name: "test_call_add",
            description: "Call add",
            parameters: {{ type: "object", properties: {{ a: {{ type: "number" }}, b: {{ type: "number" }} }} }},
            execute: async (args) => {{
                try {{
                    const res = await zier.mcp.call("mock", "add", args);
                    return JSON.stringify(res);
                }} catch (e) {{
                    return "ERROR: " + e.message;
                }}
            }}
        }});
        console.log("JS: Script finished");
    }} catch (e) {{
        console.log("JS Error: " + e);
        // We don't throw here to let load_script return successfully,
        // but the failure will be evident if tools aren't registered.
    }}
    "#, mock_server_path.to_str().unwrap().replace("\\", "\\\\"));

    let mut script_file = NamedTempFile::new().unwrap();
    script_file.write_all(script_content.as_bytes()).unwrap();
    let script_path = script_file.path().to_str().unwrap().to_string();

    service.load_script(&script_path).await.expect("Failed to load script");

    // 5. Test List Tools
    let result_list = service.execute_tool("test_list", "{}").await.expect("Failed to list tools");
    println!("List Result: {}", result_list);
    assert!(result_list.contains("echo"));
    assert!(result_list.contains("add"));

    // 6. Test Call Echo
    let result_echo = service.execute_tool("test_call_echo", r#"{"text": "hello mcp"}"#).await.expect("Failed to call echo");
    println!("Echo Result: {}", result_echo);
    assert!(result_echo.contains("hello mcp"));

    // 7. Test Call Add
    let result_add = service.execute_tool("test_call_add", r#"{"a": 10, "b": 32}"#).await.expect("Failed to call add");
    println!("Add Result: {}", result_add);
    assert!(result_add.contains("42"));
}

#[tokio::test]
async fn test_simple_ping_no_mcp() {
    use zier_alpha::scripting::deno::DenoRuntime;

    let policy = SandboxPolicy::default();
    let temp_dir = tempfile::tempdir().unwrap();

    let mut deno = DenoRuntime::new(
        policy,
        temp_dir.path().to_path_buf(),
        temp_dir.path().to_path_buf(),
        WorkdirStrategy::Overlay,
        None,
        None,
        None
    ).expect("Failed to create runtime");

    // Register simple tool
    let script = r#"
        pi.registerTool({
            name: "ping",
            description: "ping",
            parameters: {},
            execute: async () => "pong"
        });
    "#;
    // Write script to a temporary file
    let mut script_file = NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut script_file, script.as_bytes()).unwrap();
    let script_path = script_file.path().to_str().unwrap();
    deno.execute_script(script_path).await.unwrap();

    let res = deno.execute_tool("ping", "{}").await.unwrap();
    assert_eq!(res, "pong");
}
