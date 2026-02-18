use crate::agent::ImageAttachment;
use crate::config::Config;
use anyhow::Result;
use std::path::{Path, PathBuf};

// Track pending file attachments (text and images)
#[derive(Debug)]
pub enum Attachment {
    Text { name: String, content: String },
    Image { name: String, data: ImageAttachment },
    FileRef { name: String, path: String },
}

pub async fn process_attach_command(
    input: &str,
    config: &Config,
    project_dir: &Path,
    pending_attachments: &mut Vec<Attachment>,
) -> Result<String> {
    // Correctly parse path (handle spaces)
    let file_path = input[7..].trim();
    if file_path.is_empty() {
        return Err(anyhow::anyhow!("Usage: /attach <file_path>"));
    }
    let expanded = shellexpand::tilde(file_path).to_string();
    let path = Path::new(&expanded);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(file_path)
        .to_string();

    if !path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", expanded));
    }

    // Check file size
    let metadata = tokio::fs::metadata(&expanded).await?;
    let size = metadata.len();
    let max_size = config.server.attachments.max_file_size_bytes;

    // Check if it's an image file
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    let is_image = matches!(
        ext.as_deref(),
        Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("webp")
    );

    if is_image {
        if size > max_size {
            tracing::warn!(
                "Image too large ({} bytes > {} bytes). Saving as file reference.",
                size, max_size
            );
        } else {
            // Read as binary and encode as base64
            let bytes = tokio::fs::read(&expanded).await?;
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let data = STANDARD.encode(&bytes);
            let media_type = match ext.as_deref() {
                Some("png") => "image/png",
                Some("jpg") | Some("jpeg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                _ => "application/octet-stream",
            }
            .to_string();

            pending_attachments.push(Attachment::Image {
                name: filename.clone(),
                data: ImageAttachment { data, media_type },
            });
            return Ok(format!("Attached image: {} ({} bytes)", filename, size));
        }
    }

    // Non-image or large image: Handle as text or file ref
    if size > max_size {
        // Save to attachments dir
        let attach_dir = project_dir.join(&config.server.attachments.base_dir);
        if let Err(e) = tokio::fs::create_dir_all(&attach_dir).await {
            return Err(anyhow::anyhow!("Failed to create attachments directory: {}", e));
        }

        // Generate unique name
        let saved_name = format!("{}_{}", uuid::Uuid::new_v4(), filename);
        let saved_path = attach_dir.join(&saved_name);

        if let Err(e) = tokio::fs::copy(&expanded, &saved_path).await {
            return Err(anyhow::anyhow!("Failed to save attachment: {}", e));
        }

        // Add as FileRef
        let rel_path = PathBuf::from(&config.server.attachments.base_dir).join(&saved_name);

        pending_attachments.push(Attachment::FileRef {
            name: filename.clone(),
            path: rel_path.to_string_lossy().to_string(),
        });

        Ok(format!("Attached large file as reference: {} ({} bytes)\nSaved to: {}", filename, size, rel_path.display()))
    } else {
        // Read as text
        match tokio::fs::read_to_string(&expanded).await {
            Ok(content) => {
                pending_attachments.push(Attachment::Text {
                    name: filename.clone(),
                    content,
                });
                Ok(format!("Attached text: {} ({} bytes)", filename, size))
            }
            Err(_) => {
                tracing::warn!("File is not valid UTF-8. Treating as binary reference not implemented.");
                // Fallback to binary reference? (For brevity, treating as error/warning per previous logic)
                Err(anyhow::anyhow!("File is binary or invalid UTF-8 (binary fallback not implemented)"))
            }
        }
    }
}
