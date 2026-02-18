use crate::agent::tools::script::ScriptTool;
use crate::agent::tools::{create_default_tools_with_project, Tool};
use crate::agent::DiskMonitor;
use crate::config::Config;
use crate::memory::MemoryManager;
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info, warn};

pub struct ToolRegistry;

impl ToolRegistry {
    pub fn build(
        config: &Config,
        memory: Option<Arc<MemoryManager>>,
        disk_monitor: Arc<DiskMonitor>,
        script_tools: Vec<ScriptTool>,
        project_dir: PathBuf,
    ) -> Result<Vec<Arc<dyn Tool>>> {
        let mut tools_map: HashMap<String, Arc<dyn Tool>> = HashMap::new();

        // 1. Load Builtins
        let builtins =
            create_default_tools_with_project(config, memory, disk_monitor, project_dir.clone())?;

        let allowed = &config.tools.allowed_builtin;
        let allow_all = allowed.contains(&"*".to_string());

        for tool in builtins {
            if allow_all || allowed.contains(&tool.name().to_string()) {
                tools_map.insert(tool.name().to_string(), tool);
            } else {
                info!("Skipping tool '{}' (not in allowed list)", tool.name());
            }
        }

        // 2. Load External Tools
        for (name, conf) in &config.tools.external {
            let tool = crate::agent::tools::external::ExternalTool::new(
                name.clone(),
                conf.description.clone(),
                conf.command.clone(),
                conf.args.clone(),
                Some(project_dir.clone()),
                conf.sandbox,
                Some(config.sandbox.clone()),
                conf.path_args.clone(),
                Some(config.workspace_path()),
                Some(config.workdir.strategy.clone()),
            );
            tools_map.insert(name.clone(), Arc::new(tool));
        }

        // 3. Load JS Tools (overwrite builtins)
        for tool in script_tools {
            if tools_map.contains_key(tool.name()) {
                warn!("Overriding builtin tool '{}' with script tool", tool.name());
            }
            tools_map.insert(tool.name().to_string(), Arc::new(tool));
        }

        Ok(tools_map.into_values().collect())
    }
}
