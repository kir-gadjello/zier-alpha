use deno_core::error::AnyError;
use deno_core::{op2, OpState, JsRuntime, RuntimeOptions, ModuleSpecifier, v8};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;
use crate::config::SandboxPolicy;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenoToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

pub struct SandboxState {
    pub policy: SandboxPolicy,
    pub registered_tools: Vec<DenoToolDefinition>,
}

#[op2]
#[string]
pub fn op_read_file(
    state: &mut OpState,
    #[string] path: String,
) -> Result<String, std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();

    let allowed = sandbox.policy.allow_read.iter().any(|p| {
        let p_expanded = shellexpand::tilde(p).to_string();
        let path_expanded = shellexpand::tilde(&path).to_string();
        if p.ends_with('*') {
            path_expanded.starts_with(&p_expanded.trim_end_matches('*'))
        } else {
            path_expanded == p_expanded
        }
    });

    if !allowed {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Read access to {} not allowed", path)));
    }

    let content = std::fs::read_to_string(shellexpand::tilde(&path).to_string())?;
    Ok(content)
}

#[op2(fast)]
pub fn op_write_file(
    state: &mut OpState,
    #[string] path: String,
    #[string] content: String,
) -> Result<(), std::io::Error> {
    let sandbox = state.borrow::<SandboxState>();

    let allowed = sandbox.policy.allow_write.iter().any(|p| {
        let p_expanded = shellexpand::tilde(p).to_string();
        let path_expanded = shellexpand::tilde(&path).to_string();
        if p.ends_with('*') {
            path_expanded.starts_with(&p_expanded.trim_end_matches('*'))
        } else {
            path_expanded == p_expanded
        }
    });

    if !allowed {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("Write access to {} not allowed", path)));
    }

    std::fs::write(shellexpand::tilde(&path).to_string(), content)?;
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
    localgpt_ext,
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
    pub fn new(policy: SandboxPolicy) -> Result<Self, AnyError> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            extensions: vec![localgpt_ext::init_ops_and_esm()],
            ..Default::default()
        });

        let state = SandboxState {
            policy,
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
                    executeTool: async (name, args) => {
                        const tool = globalThis.toolRegistry[name];
                        if (!tool) throw new Error(`Tool ${name} not found`);
                        return await tool.execute(null, args, {}, () => {}, {});
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

        let res = self.runtime.execute_script("<tool_exec>", code)?;
        let resolve = self.runtime.resolve_value(res).await?;

        let scope = &mut self.runtime.handle_scope();
        let value = v8::Local::new(scope, resolve);

        let json = deno_core::serde_v8::from_v8::<serde_json::Value>(scope, value)?;
        if let serde_json::Value::String(s) = json {
            Ok(s)
        } else {
            Ok(json.to_string())
        }
    }
}
