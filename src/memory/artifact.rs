use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct ArtifactMetadata {
    pub id: Uuid,
    pub r#type: String,
    pub source_job: String,
    pub trust_level: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
}

pub struct ArtifactWriter {
    storage_path: PathBuf,
}

impl ArtifactWriter {
    pub fn new(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    pub async fn write(
        &self,
        content: &str,
        source_job: &str,
        trust_level: &str,
        model: &str,
    ) -> Result<PathBuf> {
        if !self.storage_path.exists() {
            fs::create_dir_all(&self.storage_path).await?;
        }

        let id = Uuid::new_v4();
        let now = Utc::now();
        let metadata = ArtifactMetadata {
            id,
            r#type: "artifact".to_string(),
            source_job: source_job.to_string(),
            trust_level: trust_level.to_string(),
            model: model.to_string(),
            created_at: now,
        };

        let yaml = serde_yaml::to_string(&metadata)?;
        let file_content = format!("---\n{}---\n\n{}", yaml, content);

        let filename = format!(
            "{}__{}__{}.md",
            now.format("%Y-%m-%d--%H-%M-%S"),
            source_job.replace('/', "_"),
            id.as_simple()
                .to_string()
                .chars()
                .take(8)
                .collect::<String>()
        );

        let path = self.storage_path.join(filename);
        fs::write(&path, file_content).await?;

        Ok(path)
    }
}
