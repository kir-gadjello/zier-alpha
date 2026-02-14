//! File system watcher for automatic memory reindexing

use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::PathBuf;

use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::MemoryIndex;
use crate::config::MemoryConfig;

pub struct MemoryWatcher {
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
    #[allow(dead_code)]
    workspace: PathBuf,
    #[allow(dead_code)]
    watched_paths: Vec<PathBuf>,
}

impl MemoryWatcher {
    pub fn new(workspace: PathBuf, db_path: PathBuf, config: MemoryConfig) -> Result<Self> {
        // Create a channel for receiving events
        let (tx, mut rx) = mpsc::channel(100);

        // Create watcher with debounce
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            match res {
                Ok(event) => {
                    // Filter for modify/create events on .md files
                    match event.kind {
                        EventKind::Modify(_) | EventKind::Create(_) => {
                            for path in event.paths {
                                if path.extension().map(|e| e == "md").unwrap_or(false) {
                                    if let Err(e) = tx.blocking_send(path.clone()) {
                                        warn!("Failed to send event: {}", e);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => warn!("Watch error: {:?}", e),
            }
        })?;

        // Watch the workspace directory
        watcher.watch(&workspace, RecursiveMode::Recursive)?;
        info!("Watching memory files in: {}", workspace.display());

        // Watch configured paths
        let mut watched_paths = vec![workspace.clone()];
        for index_path in &config.paths {
            let base_path = if index_path.path.starts_with('~') || index_path.path.starts_with('/')
            {
                PathBuf::from(shellexpand::tilde(&index_path.path).to_string())
            } else {
                workspace.join(&index_path.path)
            };

            // Skip if already watching (subdirectory of workspace)
            if base_path.starts_with(&workspace) {
                continue;
            }

            if base_path.exists() {
                if let Err(e) = watcher.watch(&base_path, RecursiveMode::Recursive) {
                    warn!("Failed to watch {}: {}", base_path.display(), e);
                } else {
                    info!("Watching configured path: {}", base_path.display());
                    watched_paths.push(base_path);
                }
            } else {
                debug!("Skipping non-existent path: {}", base_path.display());
            }
        }

        // Spawn background task to handle events
        let workspace_for_task = workspace.clone();
        let db_path_for_task = db_path.clone();
        let chunk_size = config.chunk_size;
        let chunk_overlap = config.chunk_overlap;

        tokio::spawn(async move {
            let index =
                match MemoryIndex::new_with_db_path(&workspace_for_task, &db_path_for_task, None) {
                    Ok(idx) => idx.with_chunk_config(chunk_size, chunk_overlap),
                    Err(e) => {
                        warn!("Failed to create memory index for watcher: {}", e);
                        return;
                    }
                };

            // Debounce logic is harder with async loop, but let's simplify.
            // Just process events. Maybe add a small delay/buffer if needed.
            // For now, straight processing.

            while let Some(path) = rx.recv().await {
                debug!("File changed: {}", path.display());

                // Reindex the file
                if let Err(e) = index.index_file(&path, false).await {
                    warn!("Failed to reindex file {}: {}", path.display(), e);
                } else {
                    info!("Reindexed: {}", path.display());
                }
            }
        });

        Ok(Self {
            watcher,
            workspace,
            watched_paths,
        })
    }
}
