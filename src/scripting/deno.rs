use deno_core::error::AnyError;
use deno_core::{op2, OpState, JsRuntime, RuntimeOptions, ModuleSpecifier, v8};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use std::path::PathBuf;
use crate::config::{SandboxPolicy, WorkdirStrategy};
use crate::agent::tools::resolve_path;

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

deno_core::extension!(
    zier_alpha_ext,
    ops = [
        op_read_file,
        op_write_file,
        op_fetch,
        op_log,
        op_register_tool
    ],
);

pub struct DenoRuntime {
    runtime: JsRuntime,
}

impl DenoRuntime {
    pub fn new(policy: SandboxPolicy, workspace: PathBuf, project_dir: PathBuf, strategy: WorkdirStrategy) -> Result<Self, AnyError> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![zier_alpha_ext::init_ops_and_esm()],
            ..Default::default()
        });

        let state = SandboxState {
            policy,
            workspace,
            project_dir,
            strategy,
            registered_tools: Vec::new(),
        };
        runtime.op_state().borrow_mut().put(state);

        let bootstrap_code = r#"
            globalThis.console = { log: (msg) => Deno.core.ops.op_log(String(msg)) };
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

                internal: {
                    executeTool: (name, args) => {
                        const tool = globalThis.toolRegistry[name];
                        if (!tool) throw new Error(`Tool ${name} not found`);
                        // Wrap in Promise.resolve to handle both sync and async execute functions
                        return Promise.resolve(tool.execute(null, args, {}, () => {}, {}));
                    }
                }
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
        
        // Resolve the promise by running the event loop until it settles.
        // Some promises (like those from async functions) might need multiple ticks
        // of the event loop to settle.
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
}
