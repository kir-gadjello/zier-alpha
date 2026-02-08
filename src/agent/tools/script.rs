use async_trait::async_trait;
use anyhow::Result;
use crate::agent::tools::Tool;
use crate::agent::providers::ToolSchema;
use crate::scripting::ScriptService;
use crate::scripting::deno::DenoToolDefinition;

#[derive(Clone)]
pub struct ScriptTool {
    definition: DenoToolDefinition,
    service: ScriptService,
}

impl ScriptTool {
    pub fn new(definition: DenoToolDefinition, service: ScriptService) -> Self {
        Self { definition, service }
    }
}

#[async_trait]
impl Tool for ScriptTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.definition.name.clone(),
            description: self.definition.description.clone(),
            parameters: self.definition.parameters.clone(),
        }
    }

    async fn execute(&self, arguments: &str) -> Result<String> {
        self.service.execute_tool(&self.definition.name, arguments).await
    }
}
