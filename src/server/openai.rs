use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Response,
    },
    Json,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

use crate::agent::{extract_tool_detail, Agent, AgentConfig, ContextStrategy, StreamEvent};
use crate::server::http::{AppState, SessionEntry};

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    #[serde(default)]
    pub stream: bool,
    pub user: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIChoice>,
    pub usage: OpenAIUsage,
}

#[derive(Debug, Serialize)]
pub struct OpenAIChoice {
    pub index: usize,
    pub message: OpenAIMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize, Default)]
pub struct OpenAIUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAIStreamChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamChoice {
    pub index: usize,
    pub delta: OpenAIStreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIStreamDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIModelList {
    pub object: String,
    pub data: Vec<OpenAIModel>,
}

#[derive(Debug, Serialize)]
pub struct OpenAIModel {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

#[derive(Debug, Serialize)]
pub struct OpenAIErrorResponse {
    pub error: OpenAIError,
}

#[derive(Debug, Serialize)]
pub struct OpenAIError {
    pub message: String,
    pub r#type: String,
    pub param: Option<String>,
    pub code: Option<String>,
}

fn openai_error(message: String, status: StatusCode) -> Response {
    (
        status,
        Json(OpenAIErrorResponse {
            error: OpenAIError {
                message,
                r#type: "invalid_request_error".to_string(),
                param: None,
                code: None,
            },
        }),
    )
        .into_response()
}

pub async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let res = OpenAIModelList {
        object: "list".to_string(),
        data: vec![OpenAIModel {
            id: state.config.agent.default_model.clone(),
            object: "model".to_string(),
            created: 1677610602,
            owned_by: "zier-alpha".to_string(),
        }],
    };
    Json(res).into_response()
}

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<OpenAIRequest>,
) -> Response {
    let mut message = request
        .messages
        .last()
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let mut trace_enabled = false;

    if message.starts_with("/v ") || message == "/v" {
        trace_enabled = true;
        if message.starts_with("/v ") {
            message = message[3..].to_string();
        } else {
            message = String::new();
        }
    }

    // Determine session ID: user field > Authorization header > default
    let session_id = if let Some(u) = request.user.as_ref() {
        format!("openai-{}", u)
    } else if let Some(auth_val) = headers.get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_val.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                format!(
                    "openai-{}",
                    token
                        .chars()
                        .filter(|c: &char| c.is_alphanumeric())
                        .take(20)
                        .collect::<String>()
                )
            } else {
                "openai-proxy".to_string()
            }
        } else {
            "openai-proxy".to_string()
        }
    } else {
        "openai-proxy".to_string()
    };

    let agent = {
        let mut sessions = state.sessions.lock().await;
        if let Some(e) = sessions.get_mut(&session_id) {
            e.last_accessed = Instant::now();
            e.agent.clone()
        } else {
            let agent_config = AgentConfig {
                model: state.config.agent.default_model.clone(),
                context_window: state.config.agent.context_window,
                reserve_tokens: state.config.agent.reserve_tokens,
            };

            match Agent::new(
                agent_config,
                &state.config,
                state.memory.clone(),
                ContextStrategy::Full,
            )
            .await
            {
                Ok(mut agent) => {
                    if let Err(e) = agent.new_session().await {
                        return openai_error(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR);
                    }
                    let agent_arc = Arc::new(Mutex::new(agent));
                    sessions.insert(
                        session_id.clone(),
                        SessionEntry {
                            agent: agent_arc.clone(),
                            last_accessed: Instant::now(),
                            dirty: true,
                        },
                    );
                    agent_arc
                }
                Err(e) => return openai_error(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
    };

    if request.stream {
        handle_stream(state.clone(), agent, message, trace_enabled, session_id).await
    } else {
        handle_non_stream(state.clone(), agent, message, trace_enabled, session_id).await
    }
}

async fn handle_non_stream(
    state: Arc<AppState>,
    agent: Arc<Mutex<Agent>>,
    message: String,
    trace_enabled: bool,
    session_id: String,
) -> Response {
    let mut tool_traces = Vec::new();
    let mut final_content = String::new();

    let _gate_permit = state.turn_gate.acquire().await;
    let ws_lock = state.workspace_lock.clone();
    let _ws_guard = match tokio::task::spawn_blocking(move || ws_lock.acquire()).await {
        Ok(Ok(guard)) => guard,
        _ => return openai_error("Lock error".to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut agent_lock = agent.lock().await;

    {
        let chat_res = agent_lock
            .chat_stream_with_tools(&message, Vec::new())
            .await;
        match chat_res {
            Ok(event_stream) => {
                let mut pinned_stream: Pin<
                    Box<dyn futures::Stream<Item = anyhow::Result<StreamEvent>> + Send>,
                > = Box::pin(event_stream);
                while let Some(event) = pinned_stream.next().await {
                    match event {
                        Ok(StreamEvent::Content(content)) => {
                            final_content.push_str(&content);
                        }
                        Ok(StreamEvent::ToolCallStart {
                            name,
                            id: _,
                            arguments,
                        }) => {
                            let detail = extract_tool_detail(&name, &arguments).unwrap_or_default();
                            tool_traces.push(format!("🛠️ {}: {}", name, detail));
                        }
                        Ok(StreamEvent::ToolCallEnd {
                            name: _,
                            id: _,
                            output,
                        }) => {
                            if let Some(trace) = tool_traces.last_mut() {
                                let status = if output.len() > 50 {
                                    format!(
                                        "{}...",
                                        output
                                            .chars()
                                            .take(47)
                                            .collect::<String>()
                                            .replace('\n', " ")
                                    )
                                } else {
                                    output.replace('\n', " ")
                                };
                                *trace = format!("{} -> {}", trace, status);
                            }
                        }
                        Ok(StreamEvent::Done) => break,
                        _ => {}
                    }
                }
            }
            Err(e) => {
                return openai_error(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    let usage = agent_lock.usage().clone();
    let model = agent_lock.model().to_string();
    drop(agent_lock);

    // Mark as dirty
    {
        let mut sessions = state.sessions.lock().await;
        if let Some(entry) = sessions.get_mut(&session_id) {
            entry.dirty = true;
        }
    }

    let mut response_text = String::new();
    if trace_enabled {
        response_text.push_str("┌── Tool Calls Trace ──────────────────────────────────────────\n");
        if tool_traces.is_empty() {
            response_text.push_str("│ (no tool calls)\n");
        } else {
            for trace in tool_traces {
                response_text.push_str(&format!("│ {}\n", trace));
            }
        }
        response_text
            .push_str("└──────────────────────────────────────────────────────────────\n\n");
    }
    response_text.push_str(&final_content);

    let res = OpenAIResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        model,
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content: response_text,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: OpenAIUsage {
            prompt_tokens: usage.input_tokens,
            completion_tokens: usage.output_tokens,
            total_tokens: usage.input_tokens + usage.output_tokens,
        },
    };

    Json(res).into_response()
}

async fn handle_stream(
    state: Arc<AppState>,
    agent: Arc<Mutex<Agent>>,
    message: String,
    trace_enabled: bool,
    session_id: String,
) -> Response {
    let stream = async_stream::stream! {
        let _gate_permit = state.turn_gate.acquire().await;
        let ws_lock = state.workspace_lock.clone();
        let _ws_guard = match tokio::task::spawn_blocking(move || ws_lock.acquire()).await {
            Ok(Ok(guard)) => Some(guard),
            _ => {
                let err = json!({"error": {"message": "Lock error", "type": "server_error"}});
                yield Ok::<Event, Infallible>(Event::default().data(err.to_string()));
                return;
            }
        };

        let mut agent_lock = agent.lock().await;
        let model = agent_lock.model().to_string();
        let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
        let created = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        // Send initial role
        let res = OpenAIStreamResponse {
            id: id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.clone(),
            choices: vec![OpenAIStreamChoice {
                index: 0,
                delta: OpenAIStreamDelta { role: Some("assistant".to_string()), content: None },
                finish_reason: None,
            }],
            usage: None,
        };
        yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));

        {
            let event_stream_res = agent_lock.chat_stream_with_tools(&message, Vec::new()).await;

            match event_stream_res {
                Ok(event_stream) => {
                    let mut pinned_stream: Pin<Box<dyn futures::Stream<Item = anyhow::Result<StreamEvent>> + Send>> = Box::pin(event_stream);
                    let mut trace_header_sent = false;
                    let mut has_tools = false;

                    loop {
                        let event = pinned_stream.next().await;
                        match event {
                            Some(Ok(StreamEvent::Content(content))) => {
                                if trace_enabled && !trace_header_sent {
                                    let header = "┌── Tool Calls Trace ──────────────────────────────────────────\n│ (no tool calls)\n└──────────────────────────────────────────────────────────────\n\n";
                                    let res = OpenAIStreamResponse {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![OpenAIStreamChoice {
                                            index: 0,
                                            delta: OpenAIStreamDelta { role: None, content: Some(header.to_string()) },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                                    trace_header_sent = true;
                                }
                                let res = OpenAIStreamResponse {
                                    id: id.clone(),
                                    object: "chat.completion.chunk".to_string(),
                                    created,
                                    model: model.clone(),
                                    choices: vec![OpenAIStreamChoice {
                                        index: 0,
                                        delta: OpenAIStreamDelta { role: None, content: Some(content) },
                                        finish_reason: None,
                                    }],
                                    usage: None,
                                };
                                yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                            }
                            Some(Ok(StreamEvent::ToolCallStart { name, id: _, arguments })) => {
                                has_tools = true;
                                if trace_enabled {
                                    if !trace_header_sent {
                                        let header = "┌── Tool Calls Trace ──────────────────────────────────────────\n";
                                        let res = OpenAIStreamResponse {
                                            id: id.clone(),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: model.clone(),
                                            choices: vec![OpenAIStreamChoice {
                                                index: 0,
                                                delta: OpenAIStreamDelta { role: None, content: Some(header.to_string()) },
                                                finish_reason: None,
                                            }],
                                            usage: None,
                                        };
                                        yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                                        trace_header_sent = true;
                                    }
                                    let detail = extract_tool_detail(&name, &arguments).unwrap_or_default();
                                    let trace = format!("│ 🛠️ {}: {}", name, detail);
                                    let res = OpenAIStreamResponse {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![OpenAIStreamChoice {
                                            index: 0,
                                            delta: OpenAIStreamDelta { role: None, content: Some(trace) },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                                }
                            }
                            Some(Ok(StreamEvent::ToolCallEnd { name: _, id: _, output })) => {
                                if trace_enabled {
                                    let status = if output.len() > 50 {
                                        format!(" -> {}...\n", output.chars().take(47).collect::<String>().replace('\n', " "))
                                    } else {
                                        format!(" -> {}\n", output.replace('\n', " "))
                                    };
                                    let res = OpenAIStreamResponse {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![OpenAIStreamChoice {
                                            index: 0,
                                            delta: OpenAIStreamDelta { role: None, content: Some(status) },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                                }
                            }
                            Some(Ok(StreamEvent::Done)) => {
                                if trace_enabled && trace_header_sent && has_tools {
                                    let footer = "└──────────────────────────────────────────────────────────────\n\n";
                                    let res = OpenAIStreamResponse {
                                        id: id.clone(),
                                        object: "chat.completion.chunk".to_string(),
                                        created,
                                        model: model.clone(),
                                        choices: vec![OpenAIStreamChoice {
                                            index: 0,
                                            delta: OpenAIStreamDelta { role: None, content: Some(footer.to_string()) },
                                            finish_reason: None,
                                        }],
                                        usage: None,
                                    };
                                    yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));
                                }
                                break;
                            }
                            Some(Err(e)) => {
                                let err = json!({"error": {"message": e.to_string(), "type": "server_error"}});
                                yield Ok(Event::default().data(err.to_string()));
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    let err = json!({"error": {"message": e.to_string(), "type": "server_error"}});
                    yield Ok(Event::default().data(err.to_string()));
                }
            }
        }

        let usage = agent_lock.usage().clone();
        drop(agent_lock);

        let res = OpenAIStreamResponse {
            id: id.clone(),
            object: "chat.completion.chunk".to_string(),
            created,
            model: model.clone(),
            choices: vec![OpenAIStreamChoice {
                index: 0,
                delta: OpenAIStreamDelta { role: None, content: None },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OpenAIUsage {
                prompt_tokens: usage.input_tokens,
                completion_tokens: usage.output_tokens,
                total_tokens: usage.input_tokens + usage.output_tokens,
            }),
        };
        yield Ok(Event::default().data(serde_json::to_string(&res).unwrap()));

        // Mark as dirty
        {
            let mut sessions = state.sessions.lock().await;
            if let Some(entry) = sessions.get_mut(&session_id) {
                entry.dirty = true;
            }
        }

        yield Ok(Event::default().data("[DONE]"));
    };

    Sse::new(stream).into_response()
}
