use crate::agent::{
    is_silent_reply, AgentConfig, ImageAttachment, LLMResponseContent, Message, Role,
    SessionManager, SmartClient, SmartResponse, StreamEvent, StreamResult, ToolExecutor, Usage,
    SILENT_REPLY_TOKEN,
};
use crate::capabilities::vision::VisionService;
use crate::config::Config;
use anyhow::Result;
use futures::StreamExt;
use tracing::{debug, info};

pub struct ChatEngine {
    client: SmartClient,
    session_manager: SessionManager,
    tool_executor: ToolExecutor,
    config: Config,
    agent_config: AgentConfig,
}

impl ChatEngine {
    pub fn new(
        client: SmartClient,
        session_manager: SessionManager,
        tool_executor: ToolExecutor,
        config: Config,
        agent_config: AgentConfig,
    ) -> Self {
        Self {
            client,
            session_manager,
            tool_executor,
            config,
            agent_config,
        }
    }

    pub fn client(&self) -> &SmartClient {
        &self.client
    }

    pub async fn chat(&self, message: &str) -> Result<(String, Option<Usage>)> {
        self.chat_with_images(message, Vec::new()).await
    }

    pub async fn chat_with_images(
        &self,
        message: &str,
        mut images: Vec<ImageAttachment>,
    ) -> Result<(String, Option<Usage>)> {
        let mut final_content = message.to_string();
        let config = self.client.resolve_config(&self.agent_config.model)?;

        if config.supports_vision == Some(false) && !images.is_empty() {
            info!(
                "Model {} does not support vision. Generating descriptions...",
                self.agent_config.model
            );
            let vision_service = VisionService::new(&self.config);

            for (i, img) in images.iter().enumerate() {
                match vision_service.describe_image(img).await {
                    Ok(desc) => {
                        final_content.push_str(&format!(
                            "\n\n[Image {} Description: {}]",
                            i + 1,
                            desc
                        ));
                    }
                    Err(e) => {
                        final_content.push_str(&format!(
                            "\n\n[Image {} processing failed: {}]",
                            i + 1,
                            e
                        ));
                    }
                }
            }
            images.clear();
        }

        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::User,
                content: final_content,
                tool_calls: None,
                tool_call_id: None,
                images,
            });

        if self
            .session_manager
            .should_memory_flush(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            info!("Running pre-compaction memory flush (soft threshold)");
            self.memory_flush().await?;
        }

        if self
            .session_manager
            .should_compact(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            self.session_manager.compact_session(&self.client).await?;
        }

        let messages = self
            .session_manager
            .session()
            .read()
            .await
            .messages_for_llm();
        let tool_schemas = self.tool_executor.tool_schemas();

        let response = self.client.chat(&messages, Some(&tool_schemas)).await?;

        let metadata = (response.used_model.clone(), response.latency_ms);
        let usage = response.response.usage.clone();

        let (final_response, follow_up_usage) = self.handle_response(response).await?;

        let total_usage = match (usage, follow_up_usage) {
            (Some(a), Some(b)) => Some(Usage {
                input_tokens: a.input_tokens + b.input_tokens,
                output_tokens: a.output_tokens + b.output_tokens,
            }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::Assistant,
                content: final_response.clone(),
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });

        self.session_manager
            .session()
            .write()
            .await
            .add_metadata_to_last_message(Some(metadata.0), Some(metadata.1));

        Ok((final_response, total_usage))
    }

    pub async fn handle_response(
        &self,
        response: SmartResponse,
    ) -> Result<(String, Option<Usage>)> {
        self.handle_response_internal(response).await
    }

    async fn handle_response_internal(
        &self,
        response: SmartResponse,
    ) -> Result<(String, Option<Usage>)> {
        match response.response.content {
            LLMResponseContent::Text(text) => Ok((text, None)),
            LLMResponseContent::ToolCalls(calls) => {
                self.session_manager
                    .session()
                    .write()
                    .await
                    .add_message(Message {
                        role: Role::Assistant,
                        content: String::new(),
                        tool_calls: Some(calls.clone()),
                        tool_call_id: None,
                        images: Vec::new(),
                    });

                for call in &calls {
                    debug!(
                        "Executing tool: {} with args: {}",
                        call.name, call.arguments
                    );

                    let result = self.tool_executor.execute_tool(call).await;

                    if let Err(ref e) = result {
                        if let Some(approval_err) =
                            e.downcast_ref::<crate::agent::tool_executor::ApprovalRequiredError>()
                        {
                            return Err(crate::agent::llm_error::LlmError::ApprovalRequired(
                                approval_err.0.clone(),
                                approval_err.1.clone(),
                            )
                            .into());
                        }
                    }

                    let output = result.unwrap_or_else(|e| format!("Error: {}", e));

                    // Add result incrementally so partial success is preserved
                    self.session_manager
                        .session()
                        .write()
                        .await
                        .add_message(Message {
                            role: Role::Tool,
                            content: output,
                            tool_calls: None,
                            tool_call_id: Some(call.id.clone()),
                            images: Vec::new(),
                        });
                }

                let messages = self
                    .session_manager
                    .session()
                    .read()
                    .await
                    .messages_for_llm();
                let tool_schemas = self.tool_executor.tool_schemas();

                let next_response = self.client.chat(&messages, Some(&tool_schemas)).await?;

                let usage = next_response.response.usage.clone();

                let (text, next_usage) =
                    Box::pin(self.handle_response_internal(next_response)).await?;

                let total_usage = match (usage, next_usage) {
                    (Some(a), Some(b)) => Some(Usage {
                        input_tokens: a.input_tokens + b.input_tokens,
                        output_tokens: a.output_tokens + b.output_tokens,
                    }),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                };

                Ok((text, total_usage))
            }
        }
    }

    async fn memory_flush(&self) -> Result<()> {
        self.session_manager.mark_memory_flushed().await;

        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let flush_prompt = format!(
            "Pre-compaction memory flush. Session nearing token limit.\n\
             Store durable memories now (use memory/{}.md; create memory/ if needed).\n\
             - MEMORY.md for persistent facts (user info, preferences, key decisions)\n\
             - memory/{}.md for session notes\n\n\
             If nothing to store, reply: {}",
            today, today, SILENT_REPLY_TOKEN
        );

        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::User,
                content: flush_prompt,
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });

        let messages = self
            .session_manager
            .session()
            .read()
            .await
            .messages_for_llm();
        let tool_schemas = self.tool_executor.tool_schemas();

        let response = self.client.chat(&messages, Some(&tool_schemas)).await?;
        let (final_response, _) = self.handle_response(response).await?;

        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::Assistant,
                content: final_response.clone(),
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });

        if !is_silent_reply(&final_response) {
            debug!("Memory flush response: {}", final_response);
        }

        Ok(())
    }

    pub async fn chat_stream(&self, message: &str) -> Result<StreamResult> {
        self.chat_stream_with_images(message, Vec::new()).await
    }

    pub async fn chat_stream_with_images(
        &self,
        message: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<StreamResult> {
        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::User,
                content: message.to_string(),
                tool_calls: None,
                tool_call_id: None,
                images,
            });

        if self
            .session_manager
            .should_memory_flush(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            info!("Running pre-compaction memory flush (soft threshold)");
            self.memory_flush().await?;
        }

        if self
            .session_manager
            .should_compact(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            self.session_manager.compact_session(&self.client).await?;
        }

        let messages = self
            .session_manager
            .session()
            .read()
            .await
            .messages_for_llm();
        let tool_schemas = self.tool_executor.tool_schemas();

        self.client
            .chat_stream(&messages, Some(&tool_schemas))
            .await
    }

    pub async fn finish_chat_stream(&self, response: &str) {
        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::Assistant,
                content: response.to_string(),
                tool_calls: None,
                tool_call_id: None,
                images: Vec::new(),
            });
    }

    pub async fn chat_stream_with_tools(
        &self,
        message: &str,
        images: Vec<ImageAttachment>,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>> + '_> {
        let mut final_content = message.to_string();
        let mut final_images = images;
        let config = self.client.resolve_config(&self.agent_config.model)?;

        if config.supports_vision == Some(false) && !final_images.is_empty() {
            info!(
                "Model {} does not support vision. Generating descriptions...",
                self.agent_config.model
            );
            let vision_service = VisionService::new(&self.config);

            for (i, img) in final_images.iter().enumerate() {
                match vision_service.describe_image(img).await {
                    Ok(desc) => {
                        final_content.push_str(&format!(
                            "\n\n[Image {} Description: {}]",
                            i + 1,
                            desc
                        ));
                    }
                    Err(e) => {
                        final_content.push_str(&format!(
                            "\n\n[Image {} processing failed: {}]",
                            i + 1,
                            e
                        ));
                    }
                }
            }
            final_images.clear();
        }

        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::User,
                content: final_content,
                tool_calls: None,
                tool_call_id: None,
                images: final_images,
            });

        if self
            .session_manager
            .should_memory_flush(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            info!("Running pre-compaction memory flush (soft threshold)");
            self.memory_flush().await?;
        }

        if self
            .session_manager
            .should_compact(
                self.agent_config.context_window,
                self.agent_config.reserve_tokens,
            )
            .await
        {
            self.session_manager.compact_session(&self.client).await?;
        }

        Ok(self.stream_with_tool_loop())
    }

    fn stream_with_tool_loop(&self) -> impl futures::Stream<Item = Result<StreamEvent>> + '_ {
        async_stream::stream! {
            let max_tool_iterations = 10;
            let mut iteration = 0;

            loop {
                iteration += 1;
                if iteration > max_tool_iterations {
                    yield Err(anyhow::anyhow!("Max tool iterations exceeded"));
                    break;
                }

                let tool_schemas = self.tool_executor.tool_schemas();
                let messages = self.session_manager.session().read().await.messages_for_llm();

                // Use chat_stream instead of chat
                let stream_result = self
                    .client
                    .chat_stream(&messages, Some(&tool_schemas))
                    .await;

                match stream_result {
                    Ok(mut stream) => {
                        let mut full_text = String::new();
                        let mut tool_calls = None;

                        while let Some(chunk_res) = stream.next().await {
                            match chunk_res {
                                Ok(chunk) => {
                                    if !chunk.delta.is_empty() {
                                        full_text.push_str(&chunk.delta);
                                        yield Ok(StreamEvent::Content(chunk.delta));
                                    }
                                    if chunk.done {
                                        tool_calls = chunk.tool_calls;
                                    }
                                }
                                Err(e) => {
                                    yield Err(e);
                                    return;
                                }
                            }
                        }

                        // Stream finished for this turn

                        // Add assistant message (with text and/or tool calls)
                        if !full_text.is_empty() || tool_calls.is_some() {
                             self.session_manager.session().write().await.add_message(Message {
                                role: Role::Assistant,
                                content: full_text.clone(),
                                tool_calls: tool_calls.clone(),
                                tool_call_id: None,
                                images: Vec::new(),
                            });
                        }

                        if let Some(calls) = tool_calls {
                            for call in &calls {
                                if self.tool_executor.requires_approval(&call.name) {
                                    yield Ok(StreamEvent::ApprovalRequired {
                                        name: call.name.clone(),
                                        id: call.id.clone(),
                                        arguments: call.arguments.clone(),
                                    });
                                    return;
                                }

                                yield Ok(StreamEvent::ToolCallStart {
                                    name: call.name.clone(),
                                    id: call.id.clone(),
                                    arguments: call.arguments.clone(),
                                });

                                let result = self.tool_executor.execute_tool(call).await;
                                let output = result.unwrap_or_else(|e| format!("Error: {}", e));

                                yield Ok(StreamEvent::ToolCallEnd {
                                    name: call.name.clone(),
                                    id: call.id.clone(),
                                    output: output.clone(),
                                });

                                self.session_manager.session().write().await.add_message(Message {
                                    role: Role::Tool,
                                    content: output,
                                    tool_calls: None,
                                    tool_call_id: Some(call.id.clone()),
                                    images: Vec::new(),
                                });
                            }
                            // Continue loop for next turn
                        } else {
                            // No tool calls, done
                            yield Ok(StreamEvent::Done);
                            break;
                        }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        }
    }

    pub async fn resume_chat_stream_with_tools(
        &self,
    ) -> Result<impl futures::Stream<Item = Result<StreamEvent>> + '_> {
        Ok(self.stream_with_tool_loop())
    }

    pub async fn provide_tool_result(&self, call_id: String, output: String) {
        self.session_manager
            .session()
            .write()
            .await
            .add_message(Message {
                role: Role::Tool,
                content: output,
                tool_calls: None,
                tool_call_id: Some(call_id),
                images: Vec::new(),
            });
    }

    pub async fn continue_chat(&self) -> Result<(String, Option<Usage>)> {
        let messages = self
            .session_manager
            .session()
            .read()
            .await
            .messages_for_llm();

        // Find last assistant message with tool calls
        let assistant_msg_idx = messages
            .iter()
            .rposition(|m| m.role == Role::Assistant && m.tool_calls.is_some());

        if let Some(idx) = assistant_msg_idx {
            let assistant_msg = &messages[idx];
            if let Some(calls) = &assistant_msg.tool_calls {
                // Find which calls are already done
                let executed_ids: std::collections::HashSet<_> = messages
                    .iter()
                    .skip(idx + 1)
                    .filter(|m| m.role == Role::Tool)
                    .filter_map(|m| m.tool_call_id.clone())
                    .collect();

                for call in calls {
                    if executed_ids.contains(&call.id) {
                        continue;
                    }

                    let result = self.tool_executor.execute_tool(call).await;

                    if let Err(ref e) = result {
                        if let Some(approval_err) =
                            e.downcast_ref::<crate::agent::tool_executor::ApprovalRequiredError>()
                        {
                            return Err(crate::agent::llm_error::LlmError::ApprovalRequired(
                                approval_err.0.clone(),
                                approval_err.1.clone(),
                            )
                            .into());
                        }
                    }

                    let output = result.unwrap_or_else(|e| format!("Error: {}", e));

                    self.session_manager
                        .session()
                        .write()
                        .await
                        .add_message(Message {
                            role: Role::Tool,
                            content: output,
                            tool_calls: None,
                            tool_call_id: Some(call.id.clone()),
                            images: Vec::new(),
                        });
                }

                // If we executed something or everything was already done, proceed to LLM
                // (Only proceed if all calls are done. If approval loop interrupted again, we returned Err above)
                let messages = self
                    .session_manager
                    .session()
                    .read()
                    .await
                    .messages_for_llm();
                let tool_schemas = self.tool_executor.tool_schemas();
                let response = self.client.chat(&messages, Some(&tool_schemas)).await?;

                return self.handle_response(response).await;
            }
        }
        anyhow::bail!("Nothing to continue")
    }
}
