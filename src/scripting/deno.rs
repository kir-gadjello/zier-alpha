use deno_core::error::AnyError;
use deno_core::{op2, OpState, JsRuntime, RuntimeOptions, ModuleSpecifier, v8};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::config::{SandboxPolicy, WorkdirStrategy};
use crate::agent::tools::resolve_path;
use crate::scripting::safety::{SafetyPolicy, CommandSafety};
use crate::ingress::{IngressBus, IngressMessage, TrustLevel};
use crate::scheduler::Scheduler;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenoToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct SandboxState {
    pub policy: SandboxPolicy,
    pub workspace: PathBuf,
    pub project_dir: PathBuf,
    pub strategy: WorkdirStrategy,
    pub registered_tools: Vec<DenoToolDefinition>,
    pub safety_policy: SafetyPolicy,
    pub ingress_bus: Option<Arc<IngressBus>>,
    pub scheduler: Option<Arc<Mutex<Scheduler>>>,
}

fn check_path(path: &str, allowed_paths: &[String], is_write: bool, state: &SandboxState) -> Result<PathBuf, std::io::Error> {
    let resolved_path = resolve_path(path, &state.workspace, &state.project_dir, &state.strategy);

    let abs_path = if resolved_path.exists() {
        resolved_path.canonicalize()?
    } else if is_write {
        if let Some(parent) = resolved_path.parent() {
            if parent.exists() {
                parent.canonicalize()?.join(resolved_path.file_name().ok_or(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid file name"))?)
            } else {
                return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Parent directory does not exist for {}", path)));
            }
        } else {
             resolved_path
        }
    } else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("File not found: {}", path)));
    };

    for p in allowed_paths {
        let p_expanded = shellexpand::tilde(p).to_string();
        let p_buf = PathBuf::from(&p_expanded);

        // We only allow if the allowed path exists and can be canonicalized
        if let Ok(abs_allowed) = p_buf.canonicalize() {
             if abs_path.starts_with(&abs_allowed) {
                 return Ok(abs_path);
             }
        } else {
            // If allowed path ends with *, treat as glob prefix logic on the *string* representation of absolute paths?
            // Safer to just require allowed paths to exist.
            // But if allowed is "/tmp/*", and /tmp exists:
            let p_clean = p_expanded.trim_end_matches('*');
            if let Ok(abs_allowed_base) = PathBuf::from(p_clean).canonicalize() {
                if abs_path.starts_with(&abs_allowed_base) {
                    return Ok(abs_path);
                }
            }
        }
    }

    Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Access to {} denied by policy", path)))
}

#[op2]
#[string]
pub fn op_read_file(
    state: &mut OpState,
    #[string] path: String,
) -> Result<String, std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.policy.allow_read, false, &sandbox)?;
    let content = std::fs::read_to_string(abs_path)?;
    Ok(content)
}

#[op2(fast)]
pub fn op_write_file(
    state: &mut OpState,
    #[string] path: String,
    #[string] content: String,
) -> Result<(), std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.policy.allow_write, true, &sandbox)?;
    std::fs::write(abs_path, content)?;
    Ok(())
}

#[op2(fast)]
pub fn op_fs_mkdir(
    state: &mut OpState,
    #[string] path: String,
) -> Result<(), std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.policy.allow_write, true, &sandbox)?;
    std::fs::create_dir_all(abs_path)
}

#[op2(fast)]
pub fn op_fs_remove(
    state: &mut OpState,
    #[string] path: String,
) -> Result<(), std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.policy.allow_write, true, &sandbox)?;
    if abs_path.is_dir() {
        std::fs::remove_dir_all(abs_path)
    } else {
        std::fs::remove_file(abs_path)
    }
}

#[op2(fast)]
pub fn op_write_file_exclusive(
    state: &mut OpState,
    #[string] path: String,
    #[string] content: String,
) -> Result<(), std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();
    let abs_path = check_path(&path, &sandbox.policy.allow_write, true, &sandbox)?;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(abs_path)?;

    use std::io::Write;
    file.write_all(content.as_bytes())?;
    Ok(())
}

#[op2(async)]
pub async fn op_sleep(#[serde] ms: u64) {
    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
}

#[op2]
#[string]
pub fn op_random_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[op2(async)]
#[string]
pub async fn op_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
) -> Result<String, std::io::Error> {
    {
        let state = state.borrow();
        let sandbox = state.borrow::<SandboxState>();
        if !sandbox.policy.allow_network {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Network access not allowed"));
        }
    }

    let body = reqwest::get(&url)
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
        .text()
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    Ok(body)
}

#[op2(fast)]
pub fn op_log(
    #[string] msg: String,
) {
    tracing::info!("[JS] {}", msg);
}

#[op2]
pub fn op_register_tool(
    state: &mut OpState,
    #[serde] definition: DenoToolDefinition,
) {
    let sandbox = state.borrow_mut::<SandboxState>();
    sandbox.registered_tools.push(definition);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecOpts {
    pub cwd: Option<String>,
    pub env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[op2(async)]
#[serde]
pub async fn op_zier_exec(
    state: Rc<RefCell<OpState>>,
    #[serde] cmd: Vec<String>,
    #[serde] opts: ExecOpts,
) -> Result<ExecResult, std::io::Error> {
    let (policy, project_dir) = {
        let state = state.borrow();
        let sandbox = state.borrow::<SandboxState>();
        (sandbox.safety_policy.check_command(&cmd, opts.cwd.as_deref().map(Path::new)).map_err(|e: anyhow::Error| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?, sandbox.project_dir.clone())
    };

    match policy {
        CommandSafety::Allowed => {},
        CommandSafety::SoftBlock(msg) => {
            tracing::warn!("Soft block triggered: {}", msg);
        },
        CommandSafety::RequireApproval(msg) => {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Command requires approval: {}", msg)));
        },
        CommandSafety::HardBlock(msg) => {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Command blocked by safety policy: {}", msg)));
        },
    }

    let mut command = tokio::process::Command::new(&cmd[0]);
    if cmd.len() > 1 {
        command.args(&cmd[1..]);
    }

    if let Some(path) = opts.cwd {
        let abs_cwd = if Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            project_dir.join(&path)
        };
        command.current_dir(abs_cwd);
    } else {
        command.current_dir(&project_dir);
    }

    if let Some(env) = opts.env {
        // Validate env vars
        for key in env.keys() {
            let key_upper = key.to_uppercase();
            if key_upper == "PATH" || key_upper == "HOME" || key_upper.starts_with("LD_") || key_upper == "SHELL" || key_upper == "PYTHONPATH" {
                return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Setting environment variable {} is not allowed", key)));
            }
        }
        command.envs(env);
    }

    // Capture output
    let output = command.output().await?;

    Ok(ExecResult {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

#[op2(async)]
pub async fn op_zier_ingress_push(
    state: Rc<RefCell<OpState>>,
    #[string] payload: String,
    #[string] source: String,
) -> Result<(), std::io::Error> {
    let bus = {
        let state = state.borrow();
        let sandbox = state.borrow::<SandboxState>();
        sandbox.ingress_bus.clone()
    };

    if let Some(bus) = bus {
        let msg = IngressMessage::new(source, payload, TrustLevel::TrustedEvent);
        bus.push(msg).await.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Ingress bus not available"))
    }
}

#[op2(async)]
pub async fn op_zier_scheduler_register(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[string] cron: String,
    #[string] script_path: String,
) -> Result<(), std::io::Error> {
    let (scheduler, sandbox) = {
        let state = state.borrow();
        let sandbox = state.borrow::<SandboxState>();
        (sandbox.scheduler.clone(), sandbox.clone()) // Clone needed parts or entire state if cheap
    };

    // Validate script_path
    // Use check_path with allow_read logic
    match check_path(&script_path, &sandbox.policy.allow_read, false, &sandbox) {
        Ok(_) => {},
        Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Script path blocked: {}", e))),
    }

    if let Some(scheduler) = scheduler {
        let scheduler = scheduler.lock().await;
        scheduler.register_dynamic_job(name, cron, script_path).await.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "Scheduler not available"))
    }
}

// Needed to make SandboxState cloneable for above usage or just re-borrow
impl Clone for SandboxState {
    fn clone(&self) -> Self {
        Self {
            policy: self.policy.clone(),
            workspace: self.workspace.clone(),
            project_dir: self.project_dir.clone(),
            strategy: self.strategy.clone(),
            registered_tools: self.registered_tools.clone(),
            safety_policy: SafetyPolicy {
                project_dir: self.safety_policy.project_dir.clone(),
                workspace_dir: self.safety_policy.workspace_dir.clone(),
                allow_shell_chaining: self.safety_policy.allow_shell_chaining,
                allow_global_cwd: self.safety_policy.allow_global_cwd,
            },
            ingress_bus: self.ingress_bus.clone(),
            scheduler: self.scheduler.clone(),
        }
    }
}

deno_core::extension!(
    zier_alpha_ext,
    ops = [
        op_read_file,
        op_write_file,
        op_fs_mkdir,
        op_fs_remove,
        op_write_file_exclusive,
        op_sleep,
        op_random_uuid,
        op_fetch,
        op_log,
        op_register_tool,
        op_zier_exec,
        op_zier_ingress_push,
        op_zier_scheduler_register
    ],
);

pub struct DenoRuntime {
    runtime: JsRuntime,
}

impl DenoRuntime {
    pub fn new(
        policy: SandboxPolicy,
        workspace: PathBuf,
        project_dir: PathBuf,
        strategy: WorkdirStrategy,
        ingress_bus: Option<Arc<IngressBus>>,
        scheduler: Option<Arc<Mutex<Scheduler>>>
    ) -> Result<Self, AnyError> {
        let loader = std::rc::Rc::new(deno_core::FsModuleLoader);
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(loader),
            extensions: vec![zier_alpha_ext::init_ops_and_esm()],
            ..Default::default()
        });

        let safety_policy = SafetyPolicy::new(project_dir.clone(), workspace.clone());

        let state = SandboxState {
            policy,
            workspace,
            project_dir,
            strategy,
            registered_tools: Vec::new(),
            safety_policy,
            ingress_bus,
            scheduler,
        };
        runtime.op_state().borrow_mut().put(state);

        let bootstrap_code = r#"
            globalThis.console = { log: (msg) => Deno.core.ops.op_log(String(msg)) };
            globalThis.setTimeout = (callback, delay) => {
                Deno.core.ops.op_sleep(delay).then(callback);
                return 0; // dummy handle
            };
            globalThis.crypto = {
                randomUUID: () => Deno.core.ops.op_random_uuid()
            };

            globalThis.toolRegistry = {};
            globalThis.pi = {
                registerTool: (def) => {
                    globalThis.toolRegistry[def.name] = def;
                    const meta = { ...def, execute: undefined };
                    Deno.core.ops.op_register_tool(meta);
                },
                readFile: (path) => Deno.core.ops.op_read_file(path),
                writeFile: (path, content) => Deno.core.ops.op_write_file(path, content),
                fetch: (url) => Deno.core.ops.op_fetch(url),

                fileSystem: {
                    mkdir: (path) => Deno.core.ops.op_fs_mkdir(path),
                    remove: (path) => Deno.core.ops.op_fs_remove(path),
                    writeFileExclusive: (path, content) => Deno.core.ops.op_write_file_exclusive(path, content)
                },

                internal: {
                    executeTool: (name, args) => {
                        const tool = globalThis.toolRegistry[name];
                        if (!tool) throw new Error(`Tool ${name} not found`);
                        // Wrap in Promise.resolve to handle both sync and async execute functions
                        return Promise.resolve(tool.execute(null, args, {}, () => {}, {}));
                    }
                }
            };

            // Zier Alpha Namespace
            globalThis.zier = {
                os: {
                    exec: (cmd, opts) => Deno.core.ops.op_zier_exec(cmd, opts || {})
                },
                ingress: {
                    push: (payload, source) => Deno.core.ops.op_zier_ingress_push(payload, source || "script")
                },
                scheduler: {
                    register: (name, cron, script_path) => Deno.core.ops.op_zier_scheduler_register(name, cron, script_path)
                },
                hooks: {
                    on_status: undefined
                },
                workspace: null
            };
        "#;
        runtime.execute_script("<bootstrap>", bootstrap_code)?;

        Ok(Self { runtime })
    }

    pub async fn execute_script(&mut self, path: &str) -> Result<(), AnyError> {
        let code = std::fs::read_to_string(path)?;
        let module_specifier = ModuleSpecifier::parse(&format!("file://{}", path))?;

        let mod_id = self.runtime.load_main_es_module_from_code(&module_specifier, code).await?;
        let _ = self.runtime.mod_evaluate(mod_id).await?;
        self.runtime.run_event_loop(Default::default()).await?;

        Ok(())
    }

    pub fn get_registered_tools(&mut self) -> Vec<DenoToolDefinition> {
        let state = self.runtime.op_state();
        let state = state.borrow();
        let sandbox = state.borrow::<SandboxState>();
        sandbox.registered_tools.clone()
    }

    pub async fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, AnyError> {
        let code = format!(
            "globalThis.pi.internal.executeTool('{}', {})",
            name, args
        );

        let promise_global = self.runtime.execute_script("<tool_exec>", code)?;
        
        let result_global = loop {
            let state = {
                let scope = &mut self.runtime.handle_scope();
                let promise_local = v8::Local::new(scope, &promise_global);
                let promise = v8::Local::<v8::Promise>::try_from(promise_local)?;
                
                match promise.state() {
                    v8::PromiseState::Pending => None,
                    v8::PromiseState::Fulfilled => {
                        let value = promise.result(scope);
                        Some(Ok(v8::Global::new(scope, value)))
                    }
                    v8::PromiseState::Rejected => {
                        let exception = promise.result(scope);
                        let msg = exception.to_rust_string_lossy(scope);
                        Some(Err(anyhow::anyhow!("Promise rejected: {}", msg)))
                    }
                }
            };

            if let Some(res) = state {
                break res?;
            }

            self.runtime.run_event_loop(Default::default()).await?;
        };

        let scope = &mut self.runtime.handle_scope();
        let value = v8::Local::new(scope, result_global);

        let json = deno_core::serde_v8::from_v8::<serde_json::Value>(scope, value)?;
        if let serde_json::Value::String(s) = json {
            Ok(s)
        } else {
            Ok(json.to_string())
        }
    }

    pub async fn get_status(&mut self) -> Result<Vec<String>, AnyError> {
        // Execute the hook
        let code = "globalThis.zier.hooks.on_status ? globalThis.zier.hooks.on_status() : []";

        let promise_global = self.runtime.execute_script("<get_status>", code.to_string())?;

        let result_global = loop {
             let state = {
                let scope = &mut self.runtime.handle_scope();
                let promise_local = v8::Local::new(scope, &promise_global);
                let promise = v8::Local::<v8::Promise>::try_from(promise_local)?;

                match promise.state() {
                    v8::PromiseState::Pending => None,
                    v8::PromiseState::Fulfilled => {
                        let value = promise.result(scope);
                        Some(Ok(v8::Global::new(scope, value)))
                    }
                    v8::PromiseState::Rejected => {
                        let exception = promise.result(scope);
                        let msg = exception.to_rust_string_lossy(scope);
                        Some(Err(anyhow::anyhow!("Promise rejected: {}", msg)))
                    }
                }
            };

            if let Some(res) = state {
                break res?;
            }

            self.runtime.run_event_loop(Default::default()).await?;
        };

        let scope = &mut self.runtime.handle_scope();
        let value = v8::Local::new(scope, result_global);

        let json = deno_core::serde_v8::from_v8::<serde_json::Value>(scope, value)?;

        if let serde_json::Value::Array(arr) = json {
             Ok(arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        } else if let serde_json::Value::String(s) = json {
             Ok(vec![s])
        } else {
             Ok(Vec::new())
        }
    }
}
