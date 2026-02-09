use crate::config::SandboxPolicy;
use crate::scripting::deno::{DenoRuntime, DenoToolDefinition};
use anyhow::Result;
use tokio::sync::{mpsc, oneshot};
use std::thread;
use tracing::error;

enum ScriptCommand {
    LoadScript {
        path: String,
        resp: oneshot::Sender<Result<()>>,
    },
    ExecuteTool {
        name: String,
        args: String,
        resp: oneshot::Sender<Result<String>>,
    },
    GetTools {
        resp: oneshot::Sender<Vec<DenoToolDefinition>>,
    },
}

#[derive(Clone)]
pub struct ScriptService {
    sender: mpsc::Sender<ScriptCommand>,
}

impl ScriptService {
    pub fn new(policy: SandboxPolicy) -> Result<Self> {
        let (tx, mut rx) = mpsc::channel(32);

        // Spawn a dedicated OS thread for V8 (since JsRuntime is !Send)
        thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();

            match runtime {
                Ok(rt) => {
                    rt.block_on(async move {
                        let mut deno = match DenoRuntime::new(policy) {
                            Ok(d) => d,
                            Err(e) => {
                                error!("Failed to initialize Deno runtime: {}", e);
                                return;
                            }
                        };

                        while let Some(cmd) = rx.recv().await {
                            match cmd {
                                ScriptCommand::LoadScript { path, resp } => {
                                    let res = deno.execute_script(&path).await;
                                    let _ = resp.send(res.map_err(|e| anyhow::anyhow!(e)));
                                }
                                ScriptCommand::ExecuteTool { name, args, resp } => {
                                    let res = deno.execute_tool(&name, &args).await;
                                    let _ = resp.send(res.map_err(|e| anyhow::anyhow!(e)));
                                }
                                ScriptCommand::GetTools { resp } => {
                                    let tools = deno.get_registered_tools();
                                    let _ = resp.send(tools);
                                }
                            }
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to build runtime for ScriptService: {}", e);
                }
            }
        });

        Ok(Self { sender: tx })
    }

    pub async fn load_script(&self, path: &str) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(ScriptCommand::LoadScript { path: path.to_string(), resp: tx }).await?;
        rx.await?
    }

    pub async fn execute_tool(&self, name: &str, args: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(ScriptCommand::ExecuteTool { name: name.to_string(), args: args.to_string(), resp: tx }).await?;
        rx.await?
    }

    pub async fn get_tools(&self) -> Result<Vec<DenoToolDefinition>> {
        let (tx, rx) = oneshot::channel();
        self.sender.send(ScriptCommand::GetTools { resp: tx }).await?;
        Ok(rx.await?)
    }
}
