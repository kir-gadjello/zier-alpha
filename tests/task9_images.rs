use anyhow::Result;
use zier_alpha::agent::{Session, Message, Role, ImageAttachment};

#[tokio::test]
async fn test_session_image_restore() -> Result<()> {
    // Setup
    let temp_dir = tempfile::tempdir()?;
    let root = temp_dir.path();

    // Create session
    let mut session = Session::new();
    let msg = Message {
        role: Role::User,
        content: "Look at this image".to_string(),
        tool_calls: None,
        tool_call_id: None,
        images: vec![ImageAttachment {
            media_type: "image/png".to_string(),
            data: "fakebase64".to_string(),
        }],
    };
    session.add_message(msg);

    // Save
    let path = root.join(format!("{}.jsonl", session.id()));
    session.save_to_path(&path).await?;
    let session_id = session.id().to_string();

    // Load
    let loaded = Session::load_file(&path, &session_id).await?;

    // Verify
    let messages = loaded.raw_messages();
    assert_eq!(messages.len(), 1);
    let loaded_msg = &messages[0].message;
    assert_eq!(loaded_msg.images.len(), 1);
    assert_eq!(loaded_msg.images[0].media_type, "image/png");
    assert_eq!(loaded_msg.images[0].data, "fakebase64");

    Ok(())
}
