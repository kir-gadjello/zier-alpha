# Telegram UX & Input Pipeline Enhancements – Task Specification

**Priority:** P1 (Critical) – Blocks improved Telegram usability  
**Target:** `main` branch (post-Hive refactor)  
**Status:** Ready for estimation & execution  
**Authors:** Staff Engineer (AI-assisted)  
**Date:** 2026‑02‑17

---

## Executive Summary

Zier Alpha’s Telegram integration currently processes raw messages with minimal preprocessing. This creates three critical UX gaps:

1. **Message fragmentation** – Telegram splits long messages into multiple short ones, overwhelming the agent with partial thoughts.
2. **Media ignorance** – Documents and voice messages are discarded; only text and photos reach the agent.
3. **Approval friction** – Tools requiring manual approval (e.g., `bash`, `write_file`) block the chat flow; Telegram users expect interactive button-based confirmations.
4. **System prompt rigidity** – Hardcoded Rust builder limits user customization.

This specification introduces five integrated features that work together to create a polished, production‑grade Telegram experience:

1. **Debounce queue** – aggregates fragmented messages (default 3s quiet period) before forwarding to agent.
2. **File attachment support** – downloads Telegram documents and injects them as context via XML armor.
3. **Audio transcription** – configurable backend (local command, OpenAI Whisper, or Gemini) to turn voice messages into text.
4. **Telegram button approvals** – interactive inline keyboards for approving/denying tool calls.
5. **User‑configurable system prompt generator** – JavaScript/Deno script hook for complete prompt customization.

All features are **backward‑compatible** and **user‑configurable** with sensible defaults. No breaking changes to existing APIs or behavior.

---

## 1. Problem Statement & Current Limitations

### 1.1 Message Debounce

When a user sends a long message (e.g., 2000 characters), Telegram may split it into 5–10 shorter updates. The current `TelegramPollingService` pushes each update immediately to the `IngressBus`. The agent processes them as separate thoughts, resulting in:

- Incoherent conversation fragments.
- Wasted tokens and context slots.
- Unnecessary tool calls based on incomplete information.

### 1.2 File Attachments

Only `text` and `photo` fields are extracted from `TelegramMessage`. Documents (`document`), generic `audio`, and `voice` messages are ignored. Users cannot share PDFs, text files, or voice notes for the agent to process.

### 1.3 Audio Input

Telegram `voice` messages (Opus‑encoded `.ogg`) and `audio` attachments have no transcription path. Even if downloaded, the agent (being text‑only) cannot consume them without a speech‑to‑text (STT) step.

### 1.4 Tool Approvals

The existing `ToolExecutor` supports `require_approval` configuration. When a tool call is flagged, `ApprovalRequiredError` is returned and the **caller** must invoke `approve_tool_call` and `continue_chat`. This works for synchronous CLI usage but fails for asynchronous Telegram chats where the user is not actively polling.

There is no mechanism to pause the agent, present an interactive approval request, and resume after user decision.

### 1.5 System Prompt Hardcoding

The system prompt is built by `src/agent/system_prompt.rs::build_system_prompt()`. Every organization, persona, and use‑case demands different tone, guardrails, and instructions. While `clone_sysprompt_followup` offers a tiny hook, it is insufficient for wholesale customization.

Users need a scriptable generator that can access all relevant context (workspace path, model, tools, status lines) and produce a tailored prompt.

---

## 2. Strategic Objectives

| Objective | Feature |
|-----------|---------|
| Aggregate fragmented Telegram messages into coherent units | Debounce queue |
| Make arbitrary files available to the agent | Attachment download + XML injection |
| Transcribe voice messages to text | Configurable STT backends |
| Enable interactive, button‑based tool approvals in Telegram | Approval coordinator + callback handling |
| Allow complete user control over system prompt content | JS generator script hook |

**Unified vision:** Telegram users should experience a responsive, context‑rich assistant that understands their full intent, sees shared files, hears voice notes, and approves dangerous actions with a single tap.

---

## 3. Non‑Goals

- **Web/HTTP ingress changes** – Debounce applies only to Telegram (polling) by default. The HTTP API remains unchanged to preserve existing integrations.
- **Multi‑modal LLM support** – Audio transcription is **text‑only** output. The agent never receives raw audio.
- **Approval persistence** – Pending approvals are stored only in memory. Daemon restart clears them (user must re‑trigger).
- **Attachment indexing** – Files are saved but not automatically indexed into memory search. Users must `read_file` explicitly.
- **Granular debounce per chat** – Debounce is global per source (chat ID). No per‑user tuning yet (future enhancement).
- **Multiple STT services** – Only one backend enabled at a time via config. No automatic fallback chain.
- **System prompt caching** – Generator script runs on every new session; no memoization of results.

---

## 4. Technical Decisions

### 4.1 Debounce Architecture

- Implemented in `src/inggress/controller.rs` **before** the agent task spawn.
- Structures:
  ```rust
  struct DebounceSession {
      buffer: Vec<IngressMessage>,
      last_update: Instant,
  }
  struct DebounceManager {
      sessions: HashMap<String, DebounceSession>, // key = source (e.g., "telegram:12345")
      config: IngressDebounceConfig,
      ticker: tokio::time::Interval,
  }
  ```
- The `ingress_loop` owns a `DebounceManager`. Instead of `tx.send(msg).await` and immediate spawn, it calls `manager.ingest(msg).await`.
- A background task (spawned once) runs the ticker; on each tick, `manager.flush_ready()` returns completed sessions; those are sent to the bus and removed.
- To avoid unbounded delay, `max_debounce_messages` and `max_debounce_chars` force flush when reached.

**Rationale:** Keeps debounce logic centralized, configurable, and testable. Does not block the ingress bus receiver.

### 4.2 Attachment Storage & Context Injection

- Files saved to `<project_dir>/attachments/telegram/` with sanitized filenames.
- The agent can access them via `read_file` using the relative path (project directory is cwd for file tools).
- XML structure mirrors existing `<external_content>` armor:
  ```xml
  <context>
    <attached-file filename="contract.pdf" mime="application/pdf" size="456789" path="attachments/telegram/123_contract.pdf"/>
  </context>
  ```
- The XML block is appended to the user message text, preserving any caption as leading text.
- If multiple attachments, one `<attached-file>` element per entry.

**Rationale:** Simple, explicit, and consistent with current sanitization infrastructure (`wrap_external_content`). Agent sees a clear instruction to read the file.

### 4.3 Audio Transcription Backends

Three independent implementations behind a trait:

```rust
trait Transcriber {
    async fn transcribe(&self, audio_path: &Path) -> Result<String>;
}
```

Implementations:

1. **LocalCommandTranscriber** – executes a shell command template with `{}` placeholder for file path; reads stdout.
2. **OpenAITranscriber** – uses `providers.openai` config; POST to `/v1/audio/transcriptions`.
3. **GeminiTranscriber** – uses Gemini REST API; requires separate API key (config `providers.gemini`).

Selection: read `server.audio.backend` from config; instantiate accordingly at `TelegramPollingService` construction. Fail‑fast if required config missing.

**Rationale:** Flexibility for diverse setups. Local command works offline with `whisper-cpp`; OpenAI is simplest; Gemini offers high quality. All produce plain text.

### 4.4 Button‑Based Approval Flow

**Components:**

- `ApprovalCoordinator` (singleton, Arc\<Mutex\<HashMap\<String, PendingApproval\>\>\>): maps `call_id` → pending request.
- `PendingApproval` holds: `chat_id`, `original_message_id`, `tool_name`, `arguments`, `tx: oneshot::Sender<ApprovalDecision>`, `timeout_at: Instant`.
- `TelegramPollingService` extended:
  - Accepts `Arc<ApprovalCoordinator>` and its own `chat_id` (known from config or first message).
  - Handles `callback_query` updates: extracts `data` ("approve:<call_id>" or "deny:<call_id>"), resolves coordinator, sends decision.
  - Sends API calls: `sendMessage` with `reply_markup` containing inline keyboard; `answerCallbackQuery` to dismiss; `editMessageText` to update status.
- `ingress_loop` modification:
  - In `TrustLevel::OwnerCommand` branch, wrap `agent.chat()` in a helper that intercepts `LlmError::ApprovalRequired`.
  - On such error:
    1. Build inline keyboard with ✅ and ❌ buttons; `callback_data = "approve:<id>"` / `"deny:<id>"`.
    2. Send approval request message to Telegram (quoted reply to user’s original or as new message).
    3. Register with coordinator: `coordinator.register(call.id, chat_id, msg_id, name, args, tx)`.
    4. `tokio::select!` on `rx` (decision) or timeout (config `server.telegram.approval.timeout_seconds`).
    5. On `Approve`: `agent.approve_tool_call(&call.id)`; on `Deny`: `agent.provide_tool_result(call.id, "User denied.")`.
    6. Call `agent.continue_chat()` to finish the turn.
    7. Edit the approval message to show result (✅ Approved or ❌ Denied).
    8. Send final agent response to Telegram.

**Background cleanup:** Spawn a task that loops every 30s, removes expired entries (`timeout_at < now`), and optionally edits stale messages to “⌛️ Timed out”.

**Rationale:** Decouples Telegram UI concerns from agent core. The coordinator is generic; any UI could use it. Ingress loop sees a synchronous‑like flow via `select!`.

### 4.5 System Prompt Generator Hook

- New config field: `agent.system_prompt_script: Option<PathBuf>` (default `None`).
- `Agent::new_session` (in `src/agent/mod.rs`) altered:
  1. Collect context into `serde_json::Value` (or a struct that serializes).
  2. If `system_prompt_script` is set **and** `script_service` is available, call:
     ```rust
     let script_result = script_service
         .evaluate_js_function(&script_path, "generateSystemPrompt", context_value)
         .await;
     ```
  3. On success, use returned string as system prompt (no additional Rust sections appended). On failure, log error and fall back to `build_system_prompt()`.
- `ScriptService::evaluate_js_function`:
  - Maintains a `HashMap<PathBuf, DenoRuntime>` cache. On first call for a path:
    - Create `DenoRuntime` with minimal permissions: read access to workspace, ability to return string.
    - `execute_script(path)`.
    - Ensure exported function `generateSystemPrompt` exists (checked at load).
  - On each call:
    - Serialize context to JS value.
    - `runtime.evaluate(format!("generateSystemPrompt({})", json_context)?`.
    - Convert result to `String`; error if not string.
- Script interface (Deno/JS):
  ```js
  export async function generateSystemPrompt(params) {
    // params: { workspace_dir, project_dir, model, tool_names[], hostname, current_time, timezone, skills_prompt, status_lines[] }
    // return string
    return `## Custom Prompt
You are ${params.hostname ? 'on ' + params.hostname : 'Zier Alpha'}.
...
`;
  }
  ```
- Important: The Rust `build_system_prompt` contains **essential safety and injection guards**. If the user script omits these, the agent may become vulnerable. **Decision:** The user script is **responsible** for including them. Documentation must emphasize this. Optionally, we can **prepend** a minimal safety notice automatically (non‑configurable), but the spec opts for full user control.

**Rationale:** Maximum flexibility with low overhead. Deno isolate is reused across sessions, avoiding startup cost. Script runs on main thread? No – uses dedicated runtime with its own thread.

---

## 5. Configuration Schema

Add the following to `src/config/mod.rs` and update `config.example.toml`.

### 5.1 Ingress Debounce

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct IngressDebounceConfig {
    #[serde(default = "default_debounce_seconds")]
    pub debounce_seconds: u64,
    #[serde(default = "default_max_debounce_messages")]
    pub max_debounce_messages: usize,
    #[serde(default = "default_max_debounce_chars")]
    pub max_debounce_chars: usize,
}

fn default_debounce_seconds() -> u64 { 3 }
fn default_max_debounce_messages() -> usize { 50 }
fn default_max_debounce_chars() -> usize { 100_000 }
```

Add to `Config` struct:

```rust
#[serde(default)]
pub ingress: IngressDebounceConfig,
```

### 5.2 Server Attachments

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AttachmentsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_file_size_bytes")]
    pub max_file_size_bytes: u64,
    #[serde(default)]
    pub base_dir: String, // relative to project_dir; default "attachments"
}

fn default_max_file_size_bytes() -> u64 { 10_485_760 } // 10 MB
```

Add to `ServerConfig`:

```rust
#[serde(default)]
pub attachments: AttachmentsConfig,
```

### 5.3 Server Audio

```rust
#[derive(Serialize, Deserialize, Clone)]
pub struct AudioConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_audio_backend")]
    pub backend: String, // "local", "openai", "gemini"
    #[serde(default)]
    pub local_command: Option<String>, // e.g., "whisper-cpp -m {} -f {}"
    #[serde(default)]
    pub openai_model: Option<String>, // whisper-1
    #[serde(default)]
    pub gemini_model: Option<String>,
    #[serde(default = "default_audio_timeout_seconds")]
    pub timeout_seconds: u64,
}

fn default_audio_backend() -> String { "local".to_string() }
fn default_audio_timeout_seconds() -> u64 { 60 }
```

Add to `ServerConfig`:

```rust
#[serde(default)]
pub audio: AudioConfig,
```

### 5.4 Telegram Approval

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct TelegramApprovalConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_approval_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_false")]
    pub auto_deny: bool, // if true, timeout automatically denies
}

fn default_approval_timeout_seconds() -> u64 { 300 }
fn default_false() -> bool { false }
```

Add to `ServerConfig`:

```rust
#[serde(default)]
pub telegram_approval: TelegramApprovalConfig,
```

### 5.5 Agent System Prompt Script

```rust
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct AgentConfig {
    // existing fields...
    #[serde(default)]
    pub system_prompt_script: Option<String>, // path to JS file
}
```

---

## 6. Component Breakdown & Implementation Tasks

### 6.1 Configuration Foundation

**Files:** `src/config/mod.rs`, `config.example.toml`

- Extend `Config` struct with `ingress: IngressDebounceConfig`.
- Extend `ServerConfig` with `attachments`, `audio`, `telegram_approval`.
- Extend `AgentConfig` with `system_prompt_script`.
- Add default functions.
- Update `config.example.toml` with documented examples for each new section.

**Rationale:** Centralizes all knobs; enables runtime access by all components.

---

### 6.2 Debounce Queue

**Files:**
- `src/ingress/controller.rs` – integrate `DebounceManager`.
- (Optional new module) `src/ingress/debounce.rs` – encapsulate logic.

**Implementation:**

1. Define `DebounceSession` and `DebounceManager` structs.
2. Methods: `ingest(&mut self, msg: IngressMessage)`, `flush_ready(&mut self) -> Vec<IngressMessage>`, `flush_all(&mut self) -> Vec<IngressMessage>`.
3. In `ingress_loop`, create `DebounceManager` with config.
4. Replace `bus.push(msg).await` with `manager.ingest(msg)`.
5. Spawn background task:
   ```rust
   let mut manager = ...;
   let bus = bus.clone();
   tokio::spawn(async move {
       let mut ticker = tokio::time::interval(Duration::from_millis(500));
       loop {
           ticker.tick().await;
           let ready = manager.flush_ready();
           if !ready.is_empty() {
               for msg in ready { bus.push(msg).await?; }
           }
       }
   });
   ```
6. Ensure `flush_all` called on shutdown (cleanup task).

**Edge Cases:**
- Source changes (e.g., different chat IDs) separated correctly.
- Payloads with images: each `IngressMessage` may have images; buffer must store full struct; when combining, concatenate image lists (preserving order).
- Max limits: if `buffer.len() > max_messages` or total chars > max_chars, flush immediately regardless of timer.

**Testing:** Unit tests for `DebounceManager` simulating message streams.

---

### 6.3 Telegram File Attachments

**Files:**
- `src/ingress/telegram_client.rs` – extend structs, add `document`, `audio`, `voice` fields.
- `src/server/telegram_polling.rs` – download and inject logic.

**Steps:**

1. Update `TelegramMessage`:
   ```rust
   pub document: Option<TelegramDocument>,
   pub audio: Option<TelegramAudio>,
   pub voice: Option<TelegramVoice>,
   ```
   With appropriate structs (mirroring Telegram Bot API: `file_id`, `file_name`, `mime_type`, `file_size`).
2. In `TelegramPollingService::handle_message`:
   - After text/photo branch, add `else if let Some(doc) = message.document { ... }`.
   - Check `config.server.attachments.enabled`.
   - Validate `doc.file_size` ≤ `attachments.max_file_size_bytes` (if size present).
   - Compute safe filename: sanitize (replace `/`, `\`, `..`); prefix with `message.message_id` and `chat_id` to avoid collisions.
   - Download via `client.get_file_download_url(&doc.file_id).await` → `reqwest::get(...).await` → stream to file:
     ```rust
     let path = project_dir.join(&config.server.attachments.base_dir).join("telegram").join(filename);
     // create dirs if needed
     ```
   - Build XML block: `<context>\n<attached-file filename="..." mime="..." size="..." path="..."/>\n</context>`.
   - Combine with caption if present: `caption_text + "\n\n" + xml`.
   - Push to bus as `IngressMessage` with text = combined string, images empty.
3. For `audio`/`voice`: similar download, but if `config.server.audio.enabled`, pass to transcriber instead of creating XML. The transcriber returns transcript; set text = transcript + optional caption.
4. Ensure `project_dir` is stored in `TelegramPollingService` (passed from `daemon.rs` where it is known).

**Testing:** Mock Telegram updates with `document` payload; verify file written and XML present in message.

---

### 6.4 Audio Transcription

**Files:**
- `src/server/audio.rs` (new) – trait and implementations.
- `src/server/telegram_polling.rs` – integrate transcriber.

**Steps:**

1. Define `AudioTranscriber` trait with `async fn transcribe(&self, path: &Path) -> Result<String>`.
2. Implement three structs:
   - `LocalCommandTranscriber` – config: `command_template: String`; `transcribe()` formats with path, runs `std::process::Command`, captures stdout.
   - `OpenAITranscriber` – uses `reqwest` and `config.providers.openai.api_key`; POST multipart to `/v1/audio/transcriptions` with `model` and `file`.
   - `GeminiTranscriber` – similar, using Gemini API (may need separate config section; for now assume OpenAI‑compatible if using OpenRouter? We'll keep it simple).
3. Factory: `fn create_transcriber(config: &Config) -> Result<Box<dyn AudioTranscriber>>` matches `config.server.audio.backend`.
4. In `TelegramPollingService::new`, construct transcriber and store.
5. In `handle_message`, when encountering `audio`/`voice`:
   - Download to temp file (via `tempfile::NamedTempFile`).
   - Call `transcriber.transcribe(temp_path).await`.
   - On success, build message: `format!("{}\n\n(caption)", transcript)`.
   - If audio disabled or transcriber fails, fall back to file attachment logic (save as file, generate XML).

**Testing:** Stub local command to echo "test transcript"; verify transcript appears in agent message.

---

### 6.5 Telegram Button Approvals

**Files:**
- `src/ingress/approval.rs` (new) – coordinator.
- `src/cli/daemon.rs` – create and share coordinator.
- `src/server/telegram_polling.rs` – button handling, send approval messages.
- `src/ingress/controller.rs` – intercept approvals in agent chat flow.

**Steps:**

1. Create `src/ingress/approval.rs`:
   ```rust
   use tokio::sync::{Mutex, oneshot};
   use std::time::{Instant, Duration};
   use std::collections::HashMap;

   #[derive(Clone, Copy, PartialEq, Eq)]
   pub enum ApprovalDecision { Approve, Deny }

   struct PendingApproval {
       chat_id: i64,
       message_id: i64,
       tool_name: String,
       arguments: String,
       tx: oneshot::Sender<ApprovalDecision>,
       timeout_at: Instant,
   }

   pub struct ApprovalCoordinator {
       pending: Mutex<HashMap<String, PendingApproval>>,
   }

   impl ApprovalCoordinator {
       pub fn new() -> Self { ... }
       pub async fn register(&self, call_id: String, chat_id: i64, message_id: i64, tool: String, args: String) -> oneshot::Receiver<ApprovalDecision> { ... }
       pub async fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> bool { ... }
       pub async fn cleanup(&self, now: Instant) -> Vec<String> { ... } // returns removed ids
   }
   ```
2. In `daemon.rs::run_daemon_server`, before spawning tasks:
   ```rust
   let approval_coord = Arc::new(ApprovalCoordinator::new());
   // pass to ingress_loop and TelegramPollingService
   ```
3. Modify `ingress_loop` signature to accept `Arc<ApprovalCoordinator>` and `config.server.telegram_approval`.
   - Inside `OwnerCommand` handling:
     ```rust
     match agent.chat(&msg.payload).await {
         Ok(resp) => send_response(...),
         Err(e) => if let Some(LlmError::ApprovalRequired(name, call)) = e.downcast_ref::<LlmError>() {
             let (tx, rx) = oneshot::channel();
             // Determine chat_id from msg.source (parse after "telegram:")
             let chat_id = msg.source.strip_prefix("telegram:").and_then(|s| s.parse().ok()).unwrap_or(0);
             // Send approval request via a channel to Telegram service? Need a way to send back.
             // Better: use a dedicated mpsc channel from ingress to Telegram service.
         }
     }
     ```
   - We need an **approval request channel**: create `approval_tx: mpsc::Sender<ApprovalRequest>` that goes to `TelegramPollingService`.
4. Define `ApprovalRequest` struct:
   ```rust
   pub struct ApprovalRequest {
       pub call_id: String,
       pub chat_id: i64,
       pub tool_name: String,
       pub arguments: String,
       pub respond_to: oneshot::Sender<ApprovalDecision>,
   }
   ```
   `TelegramPollingService` gets a `rx: mpsc::Receiver<ApprovalRequest>`.
5. In `ingress_loop`, when approval needed:
   ```rust
   let (tx, rx) = oneshot::channel();
   let req = ApprovalRequest { call_id: call.id.clone(), chat_id, tool_name: name.clone(), arguments: call.arguments.clone(), respond_to: tx };
   approval_tx.send(req).await?;
   // Wait for decision with timeout
   let decision = tokio::select! {
       res = rx => res.ok()?,
       after(Duration::from_secs(config.telegram.timeout_seconds)) => {
           if config.telegram.auto_deny { ApprovalDecision::Deny } else { continue waiting? but we timeout anyway }
       }
   };
   match decision {
       ApprovalDecision::Approve => agent.approve_tool_call(&call.id),
       ApprovalDecision::Deny => agent.provide_tool_result(call.id, "User denied."),
   }
   let final_resp = agent.continue_chat().await?;
   send_response(final_resp);
   ```
   **Wait:** `continue_chat` requires the tool result to already be in the session. `provide_tool_result` puts a Tool message; `approve_tool_call` marks it approved so the next `chat` or `continue_chat` will execute. Actually, after `approve_tool_call`, we should call `agent.continue_chat()` (the method exists). After `provide_tool_result`, also `continue_chat`.
6. `TelegramPollingService`:
   - Accept `approval_rx` channel.
   - In its main loop, also `select!` on Telegram updates **and** `approval_rx`.
   - On `ApprovalRequest`: send Telegram message with inline keyboard:
     ```json
     {
       "chat_id": chat_id,
       "text": format!("Tool `{}` requires approval:\nArguments: {}", tool_name, arguments),
       "reply_markup": {
         "inline_keyboard": [
           [{ "text": "✅ Approve", "callback_data": "approve:{call_id}" }],
           [{ "text": "❌ Deny", "callback_data": "deny:{call_id}" }]
         ]
       }
     }
     ```
     Store the returned `message_id` in `pending_approvals` map (coordinate with coordinator? Actually, coordinator already stores it).
   - On callback query:
     - Parse `callback_data` → `(decision, call_id)`.
     - Extract `callback_query_id`, `message` (with `chat` and `message_id`).
     - Answer callback query with empty or "Processing…".
     - Edit original message to show decision (✅ Approved by user or ❌ Denied).
     - Send decision to coordinator’s `PendingApproval.tx`. If closed, ignore.
7. `ApprovalCoordinator` needs to integrate: `register` returns the receiver; also stores `chat_id, message_id` for later edits. It should have a method `get_pending(&self, call_id) -> Option<PendingApproval>` for callbacks? Actually, the callback handling in `TelegramPollingService` should look up the pending approval by `call_id` to get the original `message_id` to edit, and to send decision by `tx`. We can store this in coordinator’s map.

**Refined design:** `ApprovalCoordinator` is the single source of truth. `ingress_loop` calls `coordinator.wait_for_decision(call_id, timeout) -> ApprovalDecision`. `TelegramPollingService` calls `coordinator.notify_callback(call_id, decision)` when user clicks. So:
- `coordinator.wait_for_decision(...)` creates entry if none, then `rx.recv()` with timeout.
- `coordinator.handle_callback(call_id, decision)` resolves the pending entry’s sender and returns `(chat_id, message_id)` for UI update.

This eliminates separate request channel; both sides share the coordinator via `Arc`.

**Implementation in `ApprovalCoordinator`:**
```rust
pub async fn wait_for_decision(&self, call_id: String, chat_id: i64, message_id: i64, tool: String, args: String, timeout: Duration) -> Option<ApprovalDecision> {
    let (tx, rx) = oneshot::channel();
    let entry = PendingApproval { chat_id, message_id, tool_name: tool, arguments: args, tx, timeout_at: Instant::now() + timeout };
    self.pending.lock().await.insert(call_id.clone(), entry);
    tokio::time::timeout(timeout, rx).await.ok()?? // None on timeout
}
pub async fn handle_callback(&self, call_id: &str) -> Option<(i64, i64, ApprovalDecision)> {
    let mut map = self.pending.lock().await;
    if let Some(entry) = map.remove(call_id) {
        // decision already sent? If we store decision in a separate field, we need to check
        // Actually, `handle_callback` is called **after** user clicks; we need to send decision through entry.tx.
        // But we also need to return chat_id/message_id to caller.
        // So: `entry.tx.send(decision)` is done by the callback handler, but we need to do it before removing.
        // Better: `resolve` method that takes decision and returns (chat_id, message_id) if found.
        None // we'll implement differently
    } else { None }
}
```
Actually, the callback handler should call `coordinator.resolve(call_id, decision)` which sends to `tx` and returns the `(chat_id, message_id)` from the entry. So:
```rust
pub async fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> Option<(i64, i64)> {
    let mut map = self.pending.lock().await;
    if let Some(entry) = map.remove(call_id) {
        let _ = entry.tx.send(decision); // ignore if receiver dropped
        Some((entry.chat_id, entry.message_id))
    } else { None }
}
```
And `wait_for_decision` inserts entry and waits on receiver.

**Background cleanup task:** Runs every 30s, calls `coordinator.cleanup(Instant::now())` to remove stale entries (those with `timeout_at < now` but whose `tx` hasn’t been resolved). Could also edit those Telegram messages to “Timed out”.

**Testing:** Simulate approval flow; verify agent continues correctly after button press.

---

### 6.6 System Prompt Generator

**Files:**
- `src/agent/system_prompt.rs` – rename current builder to `build_default_system_prompt`; create new `build_system_prompt_from_script` and `build_system_prompt`.
- `src/scripting/service.rs` – add `evaluate_generator` (see earlier plan).
- `src/agent/mod.rs` – modify `new_session` to use new builder.

**Steps:**

1. In `system_prompt.rs`:
   - Rename `build_system_prompt` to `build_default_system_prompt`.
   - Keep all existing logic intact.
2. Add new function:
   ```rust
   pub async fn build_system_prompt(
       params: SystemPromptParams,
       script_path: Option<&Path>,
       script_service: Option<&ScriptService>,
   ) -> Result<String> {
       if let (Some(path), Some(service)) = (script_path, script_service) {
           match service.evaluate_generator(path, "generateSystemPrompt", serde_json::to_value(params)?).await {
               Ok(prompt) => return Ok(prompt),
               Err(e) => log error and fall through;
           }
       }
       Ok(build_default_system_prompt(params))
   }
   ```
   Note: `SystemPromptParams` must be `Serialize`. It already contains mostly `&str` and `Vec<&str>`; convert to owned in the JSON value? We can create a separate owned struct for serialization to simplify.
3. In `ScriptService`:
   - Add field `generator_runtimes: RwLock<HashMap<PathBuf, DenoRuntime>>`.
   - Method `evaluate_generator(&self, script_path: &Path, func_name: &str, args: Value) -> Result<String>`:
     ```rust
     let mut runtimes = self.generator_runtimes.write().await;
     let runtime = runtimes.entry(script_path.to_path_buf()).or_insert_with(|| {
         let mut rt = DenoRuntime::new(... minimal permissions ...);
         rt.execute_script(script_path).ok()??;
         // check export exists
         rt
     });
     runtime.evaluate_generate(func_name, args).await
     ```
   - Need to add `evaluate_generate` to `DenoRuntime` (new method):
     ```rust
     pub async fn evaluate_generate(&self, func: &str, args: Value) -> Result<String> {
         // Build JS code: `generateSystemPrompt(arg)` and evaluate
         // Use `deno.eval` with context.
     }
     ```
   Alternatively, use `deno::impl_ops` to call a registered JS function directly. Simpler: `format!("{}({})", func, args_string)`, but must avoid injection. Use `deno_core::JsRuntime::execute_script` with proper `ModuleSpecifier::main_module` context. Could also use `deno_core::op2` to define an op that calls the JS function from Rust. Given complexity, a simpler approach: **Load script once and store a reference to the function**. But that’s advanced.

   **Simpler approach:** Use Deno’s `eval` with ` serde_json::to_string(&args)` embedded carefully (escaping). Since this is user‑controlled, we assume they write proper JS; we must not introduce injection. We’ll construct a script:

   ```js
   const args = $ARGS_JSON;
   return generateSystemPrompt(args);
   ```

   Evaluate that as a module. Use `deno_core::Source`:

   ```rust
   let code = format!("const args = {};\nreturn generateSystemPrompt(args);", serde_json::to_string(&args)?);
   let future = runtime.evaluate("generator_eval", code);
   let result = future.await?;
   // result is JsValue; convert to String
   ```

   We must ensure the function `generateSystemPrompt` is **already** defined in the runtime (from the loaded script). That works.

   Error handling: if script throws, return error.

4. In `Agent::new_session` (`src/agent/mod.rs`):
   - Build `SystemPromptParams` as before, but convert to owned `SystemPromptContext` struct.
   - Call `system_prompt::build_system_prompt(params, script_path, script_service)`.
   - Use returned string as full context.
5. Permissions for generator runtime: it should **not** need network or filesystem by default. But users may want `fs` access to read workspace files. We can grant **read** access to workspace based on config? Better: be permissive (like script tools). But generator is ephemeral; we can allow `read` to workspace and `env` maybe. Use `DenoRuntime::with_sandbox` like script tools. However, generator is created by `ScriptService`, which already has sandbox policy. We’ll use the same policy.

**Testing:** Write a generator that prepends `"CUSTOM: "` to default prompt; verify agent receives it.

---

## 7. Invariants to Maintain & Test

| # | Invariant | Affected Features |
|---|------------|-------------------|
| 1 | Debounced session preserves original message ordering and source metadata. | Debounce |
| 2 | Attachment XML paths are relative to project directory and point to existing files. | Attachments |
| 3 | Transcripts from audio are inserted as plain text without any hidden artifacts. | Audio |
| 4 | When an approval-required tool is invoked, the agent **never** executes it without a recorded `Approve` decision from the coordinator. | Approvals |
| 5 | The system prompt **always** includes safety instructions, either from user‑script or Rust fallback. | System Prompt |
| 6 | If user script fails to load, the agent continues to function using default prompt. | System Prompt |
| 7 | Pending approvals are cleaned up after timeout, releasing resources. | Approvals |
| 8 | Debounced messages respect trust level (owner vs untrusted). | Debounce |
| 9 | Attachments larger than configured limit are rejected with warning, not saved. | Attachments |
| 10 | Audio transcription failures fall back to file attachment (if enabled) or discard with warning. | Audio |

---

## 8. Testing Strategy

### 8.1 Unit Tests

- **DebounceManager**: simulate rapid messages; assert flush timing and limit enforcement.
- **ApprovalCoordinator**: test register, resolve, timeout, duplicate resolution, cleanup.
- **AudioTranscriber**: stub implementations; verify command formatting, request structure.

### 8.2 Integration Tests

- `tests/telegram_debounce.rs`: mock Telegram updates; send burst of messages; agent receives single combined message.
- `tests/telegram_attachments.rs`: send document; assert file exists on disk; read agent session to check XML presence.
- `tests/telegram_audio.rs`: send voice message; verify transcript in agent input (or fallback to XML if audio disabled).
- `tests/telegram_approval.rs`: trigger tool requiring approval; simulate button callback; verify agent continues with correct outcome.
- `tests/system_prompt_generator.rs`: write temp JS script that returns custom prompt; start agent; inspect system message in session.

### 8.3 Manual Verification Steps

- Telegram: send long message split manually; observe response coherence.
- Telegram: share a PDF; confirm agent can `read_file` it.
- Telegram: send voice note; check agent response based on transcript.
- Telegram: run a tool requiring approval; verify buttons appear; tap approve/deny.
- Change system prompt script; restart daemon; ensure new prompt takes effect.

---

## 9. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Debounce latency makes Telegram feel sluggish | Medium | UX | Default 3s is conservative; provide config to lower; source‑specific timers could be added later if needed |
| Large attachments fill disk | Medium | Resources | Enforce size limits; add periodic cleanup of `attachments/telegram/` older than N days (configurable) |
| Audio transcription service unreachable (network/API key missing) | Medium | Feature break | Clear error logs; fall back to file attachment; propagate failure as message to agent (“[Audio transcription failed]”) |
| Approval UI spam (many approvals) | Low | UX | Only tools listed in `require_approval` trigger; user can lower list |
| User‑supplied system prompt script removes critical safety instructions | Medium | Safety | Document importance; optionally auto‑prepend a minimal safety block (configurable: `agent.safety_preamble`) |
| Memory leak in DebounceManager (sessions unbounded) | Low | Resources | Enforce `max_debounce_messages` per source; flush oldest when limit exceeded |
| Race condition in ApprovalCoordinator (duplicate callbacks) | Low | Correctness | Use `HashMap::remove` to ensure single resolution; ignore if missing |
| Concurrency bottleneck in ScriptService generator runtime cache | Low | Performance | Use `RwLock`; per‑script isolate; cheap reads |

---

## 10. Success Criteria

- **All integration tests pass** (`cargo test --release`).
- **No warnings** (`cargo clippy`, `cargo fmt`).
- **Debounce:** A long Telegram message sent as 5 parts results in exactly **one** agent invocation with combined text.
- **Attachments:** Document appears on disk and the agent session contains the correct `<attached-file>` XML line.
- **Audio:** Voice note produces a transcript in the agent’s input; fallback works on error.
- **Approvals:** `bash` with `require_approval` sends a Telegram message with inline buttons; tapping ✅ executes; tapping ❌ sends “User denied.”; timeout auto‑denies if configured.
- **System Prompt Generator:** Custom JS script output is used verbatim as the system message; on script error, fallback prompt is used and error is logged.
- **Documentation:** `config.example.toml` fully documents new options; README has a “Telegram Advanced Features” section summarizing setup.

---

## 11. Migration & Backward Compatibility

- All new configuration options have sensible defaults; existing `config.toml` files load without changes.
- Existing tests continue to pass unchanged.
- No breaking changes to public Rust APIs.
- The agent core does not depend on the new features; they are gated behind config flags.
- If `system_prompt_script` is absent, behavior identical to before.

---

**End of Task Specification**
