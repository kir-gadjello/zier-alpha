use crate::agent::{ImageAttachment, LLMResponseContent, Message, Role, SmartClient};
use crate::config::Config;
use anyhow::Result;

pub struct VisionService {
    client: SmartClient,
    fallback_prompt: String,
}

impl VisionService {
    pub fn new(config: &Config) -> Self {
        let fallback_model = config.vision.fallback_model.clone();
        let fallback_prompt = config.vision.fallback_prompt.clone();

        Self {
            client: SmartClient::new(config.clone(), fallback_model),
            fallback_prompt,
        }
    }

    pub async fn describe_image(&self, image: &ImageAttachment) -> Result<String> {
        let message = Message {
            role: Role::User,
            content: self.fallback_prompt.clone(),
            tool_calls: None,
            tool_call_id: None,
            images: vec![image.clone()],
        };

        let response = self.client.chat(&[message], None).await?;
        match response.response.content {
            LLMResponseContent::Text(text) => Ok(text),
            _ => anyhow::bail!("Unexpected response from vision model (received tool calls)"),
        }
    }
}
