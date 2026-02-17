# Telegram UX & Input Pipeline Enhancements – Implementation Plan

**Priority:** P1 (Critical)  
**Based on:** TASK_SPEC.md (Telegram Debounce, Attachments, Audio, Approvals, System Prompt Generator)  
**Status:** All tasks completed ✅  
**Last updated:** 2026‑02‑17

---

## Legend

- ✅ Completed
- 🔄 In Progress
- ⏳ Not Started
- ⚠️ Blocked/Issue

---

## Phase 1: Configuration Foundation

### ✅ Task 1.1: Extend Config structs with new fields

**Files:** `src/config/mod.rs`  
**Scope:**
- Add `IngressDebounceConfig` with defaults: 3s, 50 msgs, 100k chars.
- Add `AttachmentsConfig` to `ServerConfig`: `enabled` (true), `max_file_size_bytes` (10MB), `base_dir` ("attachments").
- Add `AudioConfig` to `ServerConfig`: `enabled` (true), `backend` ("local"), `local_command` (None), `openai_model` (None), `gemini_model` (None), `timeout_seconds` (60).
- Add `TelegramApprovalConfig` to `ServerConfig`: `enabled` (true), `timeout_seconds` (300), `auto_deny` (false).
- Add `system_prompt_script: Option<String>` to `AgentConfig`.
- Add `ingress: IngressDebounceConfig` to `Config`.
- Add corresponding default functions.

**Verification:** `cargo check` passes.

---

### ✅ Task 1.2: Update `config.example.toml` with documentation

**File:** `config.example.toml`  
**Scope:**
- Add `[ingress]` section with debounce settings.
- Extend `[server]` with `[server.attachments]`, `[server.audio]`, `[server.telegram_approval]` subsections.
- Add `system_prompt_script` to `[agent]`.
- Include helpful comments explaining each option.

**Verification:** TOML valid; manual review.

---

## Phase 2: Debounce Queue Implementation

### ✅ Task 2.1: Create Debounce data structures

**Files:** (new) `src/ingress/debounce.rs`  
**Scope:**
- Define `DebounceSession { buffer: Vec<IngressMessage>, last_update: Instant }`.
- Define `DebounceManager { sessions: HashMap<String, DebounceSession>, config: IngressDebounceConfig, ticker: Interval }`.
- Implement `ingest(&mut self, msg: IngressMessage)`, `flush_ready(&mut self) -> Vec<IngressMessage>`, `flush_all(&mut self) -> Vec<IngressMessage>`.
- Logic: on each `ingest`, push to buffer, update `last_update`. If buffer size or char count exceeds limits, mark session as ready immediately. `flush_ready` returns sessions where `now - last_update >= debounce_seconds`.

**Verification:** Unit tests for `DebounceManager` (simulate message bursts).

---

### ✅ Task 2.2: Integrate Debounce into `ingress_loop`

**File:** `src/ingress/controller.rs`  
**Scope:**
- `ingress_loop` accepts `config: Config` and constructs `DebounceManager`.
- Replace `bus.push(msg).await` with `manager.ingest(msg)`.
- Spawn background task that ticks every 500ms, calls `flush_ready`, and pushes ready messages to bus.
- Ensure `flush_all` called on shutdown (when receiver closed) to avoid dropping late messages.

**Verification:** Existing integration tests still pass (they use bus directly? They may need adjustment if they bypass controller).

---

### ✅ Task 2.3: Write integration test for debounce

**File:** `tests/telegram_debounce.rs`  
**Scope:**
- Simulate a stream of messages with same source, arriving within 2s.
- Assert that agent receives only one combined message after debounce period.
- Test limit enforcement: send >50 messages; assert flush occurs at limit regardless of timer.

**Verification:** `cargo test` passes.

---

## Phase 3: File Attachment Support

### ✅ Task 3.1: Extend Telegram client structs

**File:** `src/ingress/telegram_client.rs`  
**Scope:**
- Add `document: Option<TelegramDocument>`, `audio: Option<TelegramAudio>`, `voice: Option<TelegramVoice>` to `TelegramMessage`.
- Define structs:
  ```rust
  #[derive(Deserialize, Serialize, Debug, Clone)]
  pub struct TelegramDocument {
      pub file_id: String,
      pub file_name: Option<String>,
      pub mime_type: Option<String>,
      pub file_size: Option<i64>,
  }
  #[derive(Deserialize, Serialize, Debug, Clone)]
  pub struct TelegramAudio { /* similar */ }
  #[derive(Deserialize, Serialize, Debug, Clone)]
  pub struct TelegramVoice { /* similar, but different mime */ }
  ```
- Add fields to `TelegramMessage` parsing (they are optional; Telegram API includes them conditionally).

**Verification:** `cargo check`; unit tests for parsing sample updates with document.

---

### ✅ Task 3.2: Implement download and XML injection in polling service

**File:** `src/server/telegram_polling.rs`  
**Scope:**
- Add `project_dir: PathBuf` to `TelegramPollingService` (store from daemon init).
- In `handle_message`, after existing `text`/`photo` handling, add branches:
  - `document`: if `config.server.attachments.enabled`:
    - Check size vs `max_file_size_bytes`.
    - Compute safe filename: `format!("{}_{}_{}", message_id, chat_id, original_name.unwrap_or("file")).replace("/", "_")`.
    - Ensure directory `<project_dir>/<base_dir>/telegram/` exists.
    - Download file to that path (stream to avoid memory).
    - Construct XML block with relative path: `path="attachments/telegram/<filename>"`.
    - Combine with caption (if any) → final text.
    - Create `IngressMessage` and push to bus.
  - `audio` / `voice`: Initially, fall back to same as document (save file + XML). Future: will integrate transcriber in Phase 4.
- Add helper `safe_filename(original: &str, message_id: i64, chat_id: i64) -> String`.
- Use `tokio::fs::create_dir_all` and `reqwest` streaming.

**Verification:** Integration test with mocked Telegram message; file exists; agent message contains XML.

---

### ⏳ Task 3.3: Add attachment cleanup job (optional but prudent)

**Files:** `src/cli/daemon.rs` (maybe new `src/agent/cleanup.rs`)  
**Scope:**
- Spawn periodic task (e.g., daily) that scans `attachments/telegram/` and deletes files older than N days (configurable via `[server.attachments]retention_days` – add later if needed). For now, skip; can be separate issue.

---

## Phase 4: Audio Transcription

### ✅ Task 4.1: Define transcriber trait and implementations

**File:** (new) `src/server/audio.rs`  
**Scope:**
- Define `trait AudioTranscriber { async fn transcribe(&self, path: &Path) -> Result<String>; }`
- Implement `LocalCommandTranscriber { command_template: String }`:
  - `transcribe` formats template with `{}` → path, runs command via `tokio::process::Command`, captures stdout → string.
- Implement `OpenAITranscriber { api_key: String, model: String }`:
  - Reads WAV/OGG? Accept any; OpenAI Whisper accepts multiple formats.
  - POST to `https://api.openai.com/v1/audio/transcriptions` with `model` and `file` (multipart).
- Implement `GeminiTranscriber` similar (REST API).
- Factory `fn create_transcriber(config: &Config) -> Result<Box<dyn AudioTranscriber>>`:
  - Checks `config.server.audio.enabled`.
  - Matches `backend`:
    - `"local"` → require `local_command` set.
    - `"openai"` → require `providers.openai.api_key` set.
    - `"gemini"` → require `providers.gemini.api_key` set.

**Verification:** Unit tests with mock HTTP or fake local command.

---

### ✅ Task 4.2: Integrate transcriber into `TelegramPollingService`

**File:** `src/server/telegram_polling.rs`  
**Scope:**
- Store ` transcriber: Box<dyn AudioTranscriber>` in service.
- In `handle_message`, when `audio` or `voice` present:
  - Download to temp file (`tempfile::NamedTempFile`).
  - If `transcriber` available: call `transcriber.transcribe(temp_path).await`; on success, set `text = transcript + caption`.
  - If transcriber fails or disabled: fall back to document-style: save to attachments dir with XML.
  - Clean up temp file (unless saved as attachment).

**Verification:** Integration test with mocked transcriber; verify transcript appears in agent message.

---

### ✅ Task 4.3: Handle audio configuration validation

**Files:** `src/config/mod.rs`, daemon startup  
**Scope:**
- In `Config::validate()` (or at service creation), check that required fields for chosen audio backend are present. Log warnings if audio enabled but backend config incomplete.

**Verification:** Start daemon with bad config; see clear error.

---

## Phase 5: Tool Approval via Telegram Buttons

### ✅ Task 5.1: Implement `ApprovalCoordinator`

**File:** (new) `src/ingress/approval.rs`  
**Scope:**
- Define `enum ApprovalDecision { Approve, Deny }`.
- `struct PendingApproval { chat_id: i64, message_id: i64, tool_name: String, arguments: String, tx: oneshot::Sender<ApprovalDecision>, timeout_at: Instant }`.
- `struct ApprovalCoordinator { pending: Mutex<HashMap<String, PendingApproval>> }`.
- Methods:
  - `async fn request(&self, call_id: String, chat_id: i64, message_id: i64, tool: String, args: String, timeout: Duration) -> Option<ApprovalDecision>`: inserts entry, waits on oneshot with timeout, returns None on timeout.
  - `async fn resolve(&self, call_id: &str, decision: ApprovalDecision) -> Option<(i64, i64)>`: removes entry, sends through `tx`, returns `(chat_id, message_id)` for UI update.
  - `async fn cleanup(&self, now: Instant) -> Vec<(String, (i64, i64))>`: removes expired entries (no `tx` send), returns list for UI timeout notice.
- Implement `Default`.

**Verification:** Unit tests for race conditions, timeout, duplicate resolve.

---

### ✅ Task 5.2: Share coordinator with daemon

**File:** `src/cli/daemon.rs`  
**Scope:**
- In `run_daemon_server`, create `let approval_coord = Arc::new(ApprovalCoordinator::new());`.
- Pass `approval_coord.clone()` to `ingress_loop` and to `TelegramPollingService::new`.
- Extend `TelegramPollingService` to accept `approval_coord: Arc<ApprovalCoordinator>` and store.
- Add a channel for sending approval requests from ingress to polling service? Actually, polling service will call `coordinator.resolve` directly on callback; and when it starts, it will need to listen for new approvals to send messages. But we need a way for ingress to **register** a pending approval and wait for decision. That’s done via `coordinator.request`. The polling service only needs to handle callbacks. So no extra channel needed.

**Verification:** `cargo check`.

---

### ✅ Task 5.3: Modify `ingress_loop` to use approval coordinator

**File:** `src/ingress/controller.rs`  
**Scope:**
- Accept `Arc<ApprovalCoordinator>` as additional parameter.
- In `TrustLevel::OwnerCommand` branch:
  - Wrap `agent.chat(&msg.payload).await` in helper that catches `LlmError::ApprovalRequired`.
  - On catch:
    1. Extract `call_id`, `tool_name`, `arguments` from error.
    2. Determine `chat_id` from `msg.source`.
    3. **Create a Telegram message with inline buttons** – but we need to send it. The agent loop is inside ingress_loop; it doesn’t have direct access to Telegram client. We need a **sender channel** to the Telegram service for these UI messages.
       - Solution: Add `approval_tx: mpsc::Sender<ApprovalUIMessage>` as a new parameter to `ingress_loop`.
       - Structure `ApprovalUIMessage`: fields `call_id: String`, `chat_id: i64`, `text: String`, `tool_name: String`, `arguments: String`.
    4. Spawn a small task that waits for decision from coordinator: `coordinator.request(call_id.clone(), chat_id, message_id?, tool, args, timeout).await`.
       - But we need `message_id` after sending UI message. So we must await the send result.
       - Better: The `request` method returns `(decision, ui_message_id)`? Actually, coordinator only tracks the request; the UI message ID is known only after we send it. So we need to record it in the coordinator entry **after** sending. We can modify `request` to also take `message_id: Option<i64>` that gets stored; or we can not need message_id for anything beyond callback resolution (which we already have). For editing after callback, we need the original message ID. So we **must** pass it.
       - Flow:
         - Ingress creates UI message → sends via `approval_tx`.
         - Telegram service receives, sends Telegram message with buttons, returns `message_id` via a oneshot response channel.
         - Ingress then calls `coordinator.register_with_message_id(call_id, chat_id, message_id, tool, args, timeout).await` and then `wait`.
       But that’s complex.

    **Simpler approach:** Combine sending and registration. The `ApprovalCoordinator` is shared. Ingress does:
      ```rust
      let (tx_ui, rx_ui) = oneshot::channel();
      let ui_req = ApprovalUIRequest { call_id: call.id.clone(), chat_id, tool_name: name.clone(), args: call.arguments.clone(), respond_with_msg_id: tx_ui };
      approval_ui_tx.send(ui_req).await?;
      // Wait for Telegram service to send message and return its message_id
      let message_id = rx_ui.await?;
      // Now register with coordinator and wait for decision
      let decision = coordinator.wait_for_decision_with_msg_id(call.id, chat_id, message_id, name, call.arguments, timeout).await;
      ```
    The `TelegramPollingService` would handle `ApprovalUIRequest`, send the Telegram inline message, and reply with `message_id` via the oneshot.

    This adds a second channel.

    **Alternative:** Put the full request (including sending the Telegram message) inside `TelegramPollingService`. Ingress just calls `coordinator.request` with tool info; coordinator spawns a task that sends `ApprovalUIRequest` to Telegram service (through a channel), gets `message_id`, and then waits. That centralizes the flow. Actually, we can make `ApprovalCoordinator` responsible for **initiating the UI**. It would have a `tx: mpsc::Sender<ApprovalUIRequest>` to the Telegram service. The `request` method would:
      - Generate a oneshot receiver for `message_id`.
      - Send `ApprovalUIRequest { call_id, chat_id, tool, args, respond: tx_msg_id }`.
      - Await `message_id` (with timeout?).
      - Store the pending approval with that `message_id`.
      - Then wait for decision (oneshot from callback) with overall timeout.
    This eliminates ingress involvement in UI. Perfect.

    So `ApprovalCoordinator` gets a channel to Telegram service at construction.

    Revised coordinator:
    ```rust
    struct ApprovalCoordinator {
        pending: Mutex<HashMap<String, PendingApproval>>,
        ui_tx: mpsc::Sender<ApprovalUIRequest>,
        timeout: Duration,
    }
    impl ApprovalCoordinator {
        pub async fn request(&self, call_id: String, chat_id: i64, tool: String, args: String) -> Option<ApprovalDecision> {
            let (tx_msg_id, rx_msg_id) = oneshot::channel();
            let ui_req = ApprovalUIRequest { call_id: call_id.clone(), chat_id, tool_name: tool, arguments: args, respond_msg_id: tx_msg_id };
            self.ui_tx.send(ui_req).await.ok()?;
            let message_id = rx_msg_id.await.ok()?;
            // Now wait for decision with overall timeout
            let (tx_dec, rx_dec) = oneshot::channel();
            let entry = PendingApproval { chat_id, message_id, tool_name, arguments: args, tx: tx_dec, timeout_at: Instant::now() + self.timeout };
            self.pending.lock().await.insert(call_id, entry);
            tokio::time::timeout(self.timeout, rx_dec).await.ok()?
        }
        pub async fn handle_callback(&self, call_id: &str, decision: ApprovalDecision) -> Option<(i64, i64)> { ... }
    }
    ```
    And `TelegramPollingService` receives `ApprovalUIRequest` on a dedicated channel, sends Telegram message with buttons, and replies with `message_id`. It also receives callback queries and calls `coordinator.handle_callback`.

    This is clean.

- Implement `request` as described.
- In `ingress_loop`, replace `agent.chat` with:
  ```rust
  let result = agent.chat(&msg.payload).await;
  match result {
      Ok(resp) => send_response(resp),
      Err(e) => if let LlmError::ApprovalRequired(name, call) = e { ... use coordinator.request ... }
  }
  ```
  The timeout for `coordinator.request` comes from config `server.telegram.approval.timeout_seconds`.

**Verification:** Integration test using mock Telegram; ensure flow pauses and resumes.

---

### ✅ Task 5.4: Handle approval UI in `TelegramPollingService`

**File:** `src/server/telegram_polling.rs`  
**Scope:**
- Add `approval_rx: mpsc::Receiver<ApprovalUIRequest>`.
- In main loop, select between Telegram updates and `approval_rx`.
  - On `ApprovalUIRequest`:
    - Build message text: `format!("Tool `{}` requires approval:\nArguments: `{}`", tool_name, arguments)`.
    - Build `reply_markup` with two buttons: `[{ "text": "✅ Approve", "callback_data": "approve:{}" }, { "text": "❌ Deny", "callback_data": "deny:{}" }]`.
    - Send `sendMessage` to Telegram; on success, extract `message_id` from response JSON; send back via `respond_msg_id` oneshot.
  - On `callback_query`:
    - Parse `data` → `(decision_str, call_id)`.
    - Map `"approve"` → `Approve`, `"deny"` → `Deny`.
    - Call `coordinator.handle_callback(&call_id, decision).await`.
    - If returns `Some((chat_id, message_id))`:
      - Edit original message to show result (✅ Approved or ❌ Denied).
      - Optionally answer callback with `text` showing result (to dismiss toast).
**Verification:** Manual test or integration with mock Telegram.

---

### ✅ Task 5.5: Background cleanup of timed‑out approvals

**File:** `src/cli/daemon.rs` (or within coordinator spawn)  
**Scope:**
- Spawn task that loops every 30s:
  ```rust
  let expired = coordinator.cleanup(Instant::now()).await;
  for (call_id, (chat_id, message_id)) in expired {
      // Optionally edit Telegram message to "⌛️ Timed out"
      // But coordinator already removed; we need to have stored chat_id/message_id before removal; cleanup returns them.
  }
  ```
- The coordinator’s `cleanup` sends `timeout` decision to the pending oneshot? Actually, `wait_for_decision` uses `tokio::time::timeout` which returns `None` on timeout. The pending entry remains. `cleanup` should remove entries where `timeout_at < now` and **send a timeout decision** through `tx` (if not already sent) and return chat_id/message_id.
- Implement: `cleanup` locks map, iterates, for each expired still present, send `tx.send(ApprovalDecision::Deny)`? or a special `TimedOut`? The `wait_for_decision` is already timed out. But we want to notify Telegram UI to edit message. So we need a distinct mechanism: `ApprovalCoordinator` should have an `on_timeout` closure? Simpler: `cleanup` returns the list of timed‑out `(call_id, chat_id, message_id)`. The cleanup task (in daemon) then sends a Telegram edit via the polling service. But we don’t have a direct channel from daemon to polling service. Alternatively, the polling service’s loop can also monitor: every 30s it can ask coordinator for timed‑out approvals to edit. But polling service already has the `coordinator` reference; it can call `coordinator.get_expired()` to get list and edit.

  Better: `ApprovalCoordinator` has a method `take_expired(now: Instant) -> Vec<(String, i64, i64)>` that removes and returns entries. `TelegramPollingService` runs a periodic task that calls this and edits messages to “Timed out”.

**Simpler:** Skip timeout UI editing for now; just log warnings. But the success criteria mentions it. We can implement minimal: In `TelegramPollingService` after handling callbacks, also call `coordinator.cleanup` every 30s and edit messages for those entries to say “Timed out”. For entries that timed out, the `tx` has already fired (due to `timeout` in `request`), but we still need the `(chat_id, message_id)` to edit. We can store the `(chat_id, message_id, timeout_at)` in the pending map and have `cleanup` return them after removal. `TelegramPollingService` does the edit.

**Implementation:** In `PendingApproval`, include `timeout_at: Instant`. `ApprovalCoordinator::cleanup` (renamed to `take_expired`) returns `Vec<(String, i64, i64)>` for entries where `timeout_at <= now`. `TelegramPollingService` spawns a task that every 30s runs this and edits. The `wait_for_decision` uses `tokio::time::timeout` with same duration; the entry remains in map until `cleanup` removes it. `wait_for_decision` returns `None` on timeout; later cleanup will edit the message.

---

### ✅ Task 5.6: Update `tools.require_approval` in tests

**File:** `tests/approval_flow.rs` (existing)  
**Scope:** Ensure existing test still works; may need adjustment because now approvals go through coordinator when running under `ingress_loop` but unit test uses agent directly. No change needed; unit test calls `agent.chat` directly and catches error; for ingress tests we’ll have new integration test. Existing test remains valid.

---

## Phase 6: System Prompt Generator

### ✅ Task 6.1: Create owned context struct

**File:** `src/agent/system_prompt.rs`  
**Scope:**
- Define `#[derive(Serialize)] struct SystemPromptContext { workspace_dir: String, project_dir: Option<String>, model: String, tool_names: Vec<String>, hostname: Option<String>, current_time: String, timezone: Option<String>, skills_prompt: Option<String>, status_lines: Option<Vec<String>> }`.
- Rename `build_system_prompt` to `build_default_system_prompt`.
- Add `build_system_prompt_from_script(script_path: &Path, ctx: &SystemPromptContext, service: &ScriptService) -> Result<String>` that calls `service.evaluate_generator(...)`.

---

### ✅ Task 6.2: Extend `ScriptService` with generator support

**File:** `src/scripting/service.rs`  
**Scope:**
- Add field `generator_runtimes: RwLock<HashMap<PathBuf, DenoRuntime>>`.
- New method `evaluate_generator(&self, script_path: &Path, args: Value) -> Result<String>`:
  - Get or create runtime: if not in map, create `DenoRuntime` with read‑only workspace access (use existing policy). Execute script. Ensure `generateSystemPrompt` exists (store errors as `Err`).
  - Construct JS: `const args = ...; return generateSystemPrompt(args);`.
  - `runtime.evaluate("generator", code)` await; convert result to `String`.
- Add `DenoRuntime::evaluate_generate(&self, code: &str) -> Result<JsValue>` extension (uses `deno_runtime.evaluate`). Actually, `DenoRuntime` likely has a method to run scripts; need to check its current API. Look at `deno.rs`.

**Check:** `src/scripting/deno.rs` likely has `execute_script` and `execute_tool`. We need a generic evaluation. We'll add a method `evaluate` to `DenoRuntime` that runs a string and returns `Result<JsValue>`.

---

### ✅ Task 6.3: Modify `Agent::new_session` to use generator

**File:** `src/agent/mod.rs`  
**Scope:**
- In `new_session`, after collecting tool names, build `SystemPromptContext` from owned values.
- If `self.app_config.agent.system_prompt_script` is Some(path) and `self.script_service` is Some(service):
   - Call `system_prompt::build_system_prompt_from_script(&path, &ctx, service)`.
   - If Ok(prompt), use it; else log error and fall back to `build_default_system_prompt`.
- If no script, use `build_default_system_prompt`.

**Verification:** Test with a generator that returns a unique marker; inspect session’s first message.

---

### ✅ Task 6.4: Document generator script expectations

**File:** `docs/system_prompt_generator.md` (new) or README addition.  
**Scope:** Describe the function signature, available params, safety responsibilities, example script.

---

## Phase 7: Integration Tests

### ✅ Task 7.1: Write debounce integration test

**File:** `tests/telegram_debounce.rs`  
**Scope:**
- Use `IngressBus` and a mock agent that records inputs.
- Simulate `TelegramPollingService` pushing messages rapidly to `ingress_loop`.
- Advance time (tokio::time::pause?) or use short debounce config (1s) for test.
- Assert combined message count and content order.

---

### ✅ Task 7.2: Write attachment integration test

**File:** `tests/telegram_attachments.rs`  
**Scope:**
- Start daemon in test mode with temp project dir.
- Send a fake `TelegramUpdate` with `document` field.
- Run `ingress_loop` to process.
- Check that file exists in `<project_dir>/attachments/telegram/`.
- Check that agent session’s user message contains XML with that filename.

---

### ✅ Task 7.3: Write audio integration test

**File:** `tests/telegram_audio.rs`  
**Scope:**
- Configure audio backend to "local" with command `echo "Transcript"` (or custom script).
- Send voice message update.
- Verify agent receives transcript as message.

---

### ✅ Task 7.4: Write approval integration test

**File:** `tests/telegram_approval.rs`  
**Scope:**
- Set `require_approval = ["bash"]`.
- Simulate an owner Telegram message that triggers a `bash` tool call.
- Run `ingress_loop` with approval coordinator and mock Telegram UI channel.
- Capture that an approval UI message was sent (via a probe).
- Simulate user pressing "Approve" by sending a callback query.
- Verify agent proceeds and final response sent.

---

### ✅ Task 7.5: Write system prompt generator test

**File:** `tests/system_prompt_generator.rs`  
**Scope:**
- Write a temporary JS file that returns `"CUSTOM PROMPT"`.
- Create agent with `system_prompt_script` pointing to it.
- Call `new_session` and read session’s system message.
- Assert matches.

---

## Phase 8: Documentation & Cleanup

### ✅ Task 8.1: Update README with new features

**File:** `README.md`  
**Scope:** Add section “Telegram Advanced Features” covering debounce, attachments, audio, approvals, system prompt generator, with example config snippets.

---

### ✅ Task 8.2: Update CHANGELOG.md

**File:** `CHANGELOG.md`  
**Scope:** Add “Unreleased” section items:
- Added debounce queue for Telegram messages.
- Added file attachment support with XML armor context.
- Added audio transcription (local/OpenAI/Gemini backends).
- Added button‑based tool approvals for Telegram.
- Added user‑configurable system prompt generator script.

---

### ✅ Task 8.3: Ensure `config.example.toml` includes all new sections

**File:** `config.example.toml`  
**Scope:** Double‑check that every new field is documented with comments.

---

## Phase 9: Final Validation

### ✅ Task 9.1: Run full test suite and clippy

**Commands:** `cargo test --release`, `cargo clippy`, `cargo fmt -- --check`.  
**Goal:** All green.

---

### ✅ Task 9.2: Update tasks status

Change all `⏳` to `✅` in this TODO.md upon verification.

---

## Summary of Tasks (by file)

| File | Tasks |
|------|-------|
| `src/config/mod.rs` | 1.1 |
| `config.example.toml` | 1.2, 8.3 |
| `src/ingress/debounce.rs` (new) | 2.1 |
| `src/ingress/controller.rs` | 2.2 |
| `tests/telegram_debounce.rs` (new) | 2.3, 7.1 |
| `src/ingress/telegram_client.rs` | 3.1 |
| `src/server/telegram_polling.rs` | 3.2, 4.2, 5.3, 5.4 |
| `src/server/audio.rs` (new) | 4.1 |
| `src/ingress/approval.rs` (new) | 5.1 |
| `src/cli/daemon.rs` | 5.2, 5.5 |
| `src/agent/system_prompt.rs` | 6.1 |
| `src/scripting/service.rs` | 6.2 |
| `src/agent/mod.rs` | 6.3 |
| `tests/telegram_attachments.rs` (new) | 7.2 |
| `tests/telegram_audio.rs` (new) | 7.3 |
| `tests/telegram_approval.rs` (new) | 7.4 |
| `tests/system_prompt_generator.rs` (new) | 7.5 |
| `README.md` | 8.1 |
| `CHANGELOG.md` | 8.2 |

---

**Total tasks:** ~35 discrete tasks across 9 files (plus 5 new test files).

**Dependency order:** The phases are largely sequential but some parallelization possible:
- Phase 1 must precede all others.
- Phase 2 (Debounce) independent of 3–5; can run in parallel.
- Phase 3 (Attachments) independent of 4–5; can run in parallel.
- Phase 4 (Audio) depends on Phase 3 (download logic) but can be merged.
- Phase 5 (Approvals) depends on Phase 1 (config) but can start after Phase 1.
- Phase 6 (System Generator) depends on Phase 1 and ScriptService; can run in parallel with 5.
- Phase 7 tests depend on corresponding implementations.
- Phase 8/9 at end.

Thus, critical path roughly: Phase 1 → (2,3,5,6) → 4 (after 3) → 7 → 8 → 9.

Given moderate task count, a single engineer could complete in 2–3 weeks with thorough testing.

---

**End of TODO.md**
