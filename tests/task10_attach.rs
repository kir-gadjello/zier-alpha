use anyhow::Result;
use std::path::PathBuf;
use zier_alpha::agent::attachments::{process_attach_command, Attachment};
use zier_alpha::config::Config;

#[tokio::test]
async fn test_process_attach_command() -> Result<()> {
    if cfg!(windows) {
        return Ok(());
    }

    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();
    let project_dir = root.to_path_buf();

    // Create config with small limit to trigger file ref
    let mut config = Config::default();
    config.server.attachments.max_file_size_bytes = 10; // 10 bytes limit

    let mut pending = Vec::new();

    // 1. Create large file
    let large_file = root.join("large.txt");
    std::fs::write(&large_file, "This is a large file > 10 bytes")?;

    // 2. Process
    let input = format!("/attach {}", large_file.to_string_lossy());
    process_attach_command(&input, &config, &project_dir, &mut pending).await?;

    // 3. Verify it's a FileRef
    assert_eq!(pending.len(), 1);
    if let Attachment::FileRef { name, path } = &pending[0] {
        assert_eq!(name, "large.txt");
        assert!(path.contains("attachments"));
        assert!(project_dir.join(path).exists());
    } else {
        panic!("Expected FileRef");
    }

    // 4. Create small file
    pending.clear();
    let small_file = root.join("small.txt");
    std::fs::write(&small_file, "small")?;

    let input = format!("/attach {}", small_file.to_string_lossy());
    process_attach_command(&input, &config, &project_dir, &mut pending).await?;

    // 5. Verify it's Text
    assert_eq!(pending.len(), 1);
    if let Attachment::Text { name, content } = &pending[0] {
        assert_eq!(name, "small.txt");
        assert_eq!(content, "small");
    } else {
        panic!("Expected Text");
    }

    // 6. Test space handling
    pending.clear();
    let space_file = root.join("file with spaces.txt");
    std::fs::write(&space_file, "small")?;

    let input = format!("/attach {}", space_file.to_string_lossy());
    // input is "/attach /tmp/.../file with spaces.txt"
    process_attach_command(&input, &config, &project_dir, &mut pending).await?;

    assert_eq!(pending.len(), 1);
    if let Attachment::Text { name, .. } = &pending[0] {
        assert_eq!(name, "file with spaces.txt");
    } else {
        panic!("Expected Text for spaced file");
    }

    Ok(())
}
