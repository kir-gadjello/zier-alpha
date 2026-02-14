use anyhow::Result;
use glob::glob;
use std::collections::HashMap;
use std::path::Path;
use tracing::{info, warn};

pub struct PromptRegistry {
    prompts: HashMap<String, String>,
}

impl PromptRegistry {
    pub fn new() -> Self {
        Self {
            prompts: HashMap::new(),
        }
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> Result<()> {
        if !dir.exists() {
            warn!("Prompt directory not found: {}", dir.display());
            return Ok(());
        }

        let pattern = dir.join("**/*.md");
        let pattern_str = pattern
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid path"))?;

        for entry in glob(pattern_str)? {
            match entry {
                Ok(path) => {
                    // Calculate ID relative to dir
                    if let Ok(relative_path) = path.strip_prefix(dir) {
                        let id = relative_path
                            .with_extension("")
                            .to_string_lossy()
                            .to_string();
                        // Normalize windows paths if any
                        let id = id.replace('\\', "/");

                        let content = std::fs::read_to_string(&path)?;
                        self.prompts.insert(id.clone(), content);
                        info!("Loaded prompt: {}", id);
                    }
                }
                Err(e) => warn!("Error reading prompt file: {}", e),
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&String> {
        self.prompts.get(id)
    }
}

impl Default for PromptRegistry {
    fn default() -> Self {
        Self::new()
    }
}
