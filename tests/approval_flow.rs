use async_trait::async_trait;
use std::sync::Arc;
use tempfile::TempDir;
use zier_alpha::agent::{Agent, AgentConfig, ContextStrategy, Tool, ToolSchema};
use zier_alpha::config::Config;
use zier_alpha::memory::MemoryManager;

struct MockTool;
#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        "test_write"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "test_write".into(),
            description: "Mock tool".into(),
            parameters: serde_json::json!({}),
        }
    }
    async fn execute(&self, _args: &str) -> anyhow::Result<String> {
        Ok("Tool execution verified".to_string())
    }
}

#[tokio::test]
async fn test_approval_flow_non_streaming() {
    let temp_dir = TempDir::new().unwrap();
    let workspace_path = temp_dir.path().to_path_buf();

    let mut config = Config::default();
    config.memory.workspace = workspace_path.to_string_lossy().to_string();
    config.agent.default_model = "mock/test".to_string();

    // Require approval for test_write
    config.tools.require_approval = vec!["test_write".to_string()];
    config.tools.allowed_builtin = vec!["*".to_string()];

    let memory =
        MemoryManager::new_with_full_config(&config.memory, Some(&config), "test-agent").unwrap();

    let agent_config = AgentConfig {
        model: "mock/test".to_string(),
        context_window: 100000,
        reserve_tokens: 1000,
    };

    let mut agent = Agent::new(agent_config, &config, memory, ContextStrategy::Full, "test")
        .await
        .unwrap();

    // Add MockTool
    let mut tools = agent.tools().to_vec();
    tools.push(Arc::new(MockTool));
    agent.set_tools(tools);

    agent.new_session().await.unwrap();

    // Trigger tool call: "test_tool:test_write|path|content"
    let result = agent.chat("test_tool:test_write|test.txt|content").await;

    match result {
        Ok(_) => panic!("Should have failed with ApprovalRequired"),
        Err(e) => {
            if let Some(err) = e.downcast_ref::<zier_alpha::agent::LlmError>() {
                match err {
                    zier_alpha::agent::LlmError::ApprovalRequired(name, call) => {
                        assert_eq!(name, "test_write");

                        // Approve and continue
                        agent.approve_tool_call(&call.id);

                        let retry = agent.continue_chat().await;
                        if retry.is_err() {
                            panic!("Retry failed: {:?}", retry.err());
                        }
                        let output = retry.unwrap();

                        // MockProvider returns "Mock response" or "Tool execution verified" depending on how it's called
                        // In MockProvider::chat:
                        // if last_msg.role == Tool { if content starts with Error return it }
                        // if test_tool_json ...
                        // if test_tool ...
                        // if write_memory_msg ...
                        // else return "Mock response"

                        // When continuing chat, Tool result is added.
                        // Then client.chat is called.
                        // MockProvider sees messages including Tool message.
                        // The MockProvider logic for `test_tool` checks if last message is NOT tool result to return tool call.
                        // If it IS tool result, it falls through to "Mock response".
                        // Wait, my MockProvider logic:
                        // if let Some(tool_req) = last_msg.content.strip_prefix("test_tool:") ...
                        //    if !last_msg.content.contains("tool_result") && last_msg.role != Role::Tool { ... return ToolCall }
                        //    return Ok(LLMResponse::text("Tool execution verified"));

                        // BUT, when continue_chat calls client.chat, the LAST message is the Tool message (role=Tool).
                        // So `last_msg.content` is the tool output ("Tool execution verified").
                        // It does NOT start with "test_tool:".
                        // So it falls through to "Mock response".

                        // I should update MockProvider to recognize the tool output?
                        // Or just assert that it succeeded.
                        // "Mock response" means the LLM responded after the tool execution.
                        // Which implies the tool execution was successful and fed back to LLM.

                        // Let's accept "Mock response" as success here, or check if the tool output is in history.
                        assert_eq!(output, "Mock response");

                        // Verify tool output is in session history
                        let messages = agent.session_messages().await;
                        let tool_msg = messages
                            .iter()
                            .find(|m| m.role == zier_alpha::agent::Role::Tool)
                            .expect("Tool message missing");
                        assert!(tool_msg.content.contains("Tool execution verified"));
                    }
                    _ => panic!("Wrong error type: {:?}", err),
                }
            } else {
                panic!("Wrong error type (anyhow wrapping): {:?}", e);
            }
        }
    }
}
