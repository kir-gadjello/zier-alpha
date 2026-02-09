use crate::scripting::ScriptService;
use anyhow::Result;
use std::path::Path;
use tracing::{info, warn};
use glob::glob;

pub struct ScriptLoader {
    service: ScriptService,
}

impl ScriptLoader {
    pub fn new(service: ScriptService) -> Self {
        Self { service }
    }

    pub async fn load_from_dir(&self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        let pattern = dir.join("*.js");
        let pattern_str = pattern.to_str().ok_or_else(|| anyhow::anyhow!("Invalid path"))?;

        for entry in glob(pattern_str)? {
            match entry {
                Ok(path) => {
                    let path_str = path.to_string_lossy();
                    info!("Loading script: {}", path_str);
                    if let Err(e) = self.service.load_script(&path_str).await {
                        warn!("Failed to load script {}: {}", path_str, e);
                    }
                }
                Err(e) => warn!("Error listing script file: {}", e),
            }
        }
        Ok(())
    }
}
