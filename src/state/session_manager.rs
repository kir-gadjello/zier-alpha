use crate::agent::session::Session;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};
use std::time::Duration;

#[derive(Clone)]
pub struct GlobalSessionManager {
    // Map of session ID -> Session
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<Session>>>>>,
}

impl GlobalSessionManager {
    pub fn new() -> Self {
        let manager = Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        };

        manager.start_auto_save_task();
        manager
    }

    /// Get existing session or load from disk.
    /// Returns None if not found.
    pub async fn get_session(&self, id: &str) -> Result<Option<Arc<RwLock<Session>>>> {
        // 1. Check memory
        {
            let sessions = self.sessions.read().await;
            if let Some(session) = sessions.get(id) {
                return Ok(Some(session.clone()));
            }
        } // Drop read lock

        // 2. Load from disk (requires write lock on map to insert)
        // Check again after acquiring write lock to avoid race
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get(id) {
            return Ok(Some(session.clone()));
        }

        // Try load from disk
        match Session::load(id) {
            Ok(session) => {
                let session_arc = Arc::new(RwLock::new(session));
                sessions.insert(id.to_string(), session_arc.clone());
                info!("Loaded session {} from disk", id);
                Ok(Some(session_arc))
            }
            Err(_) => Ok(None),
        }
    }

    /// Get existing or create new session
    pub async fn get_or_create_session(&self, id: &str) -> Result<Arc<RwLock<Session>>> {
        if let Some(session) = self.get_session(id).await? {
            return Ok(session);
        }

        // Create new
        let mut sessions = self.sessions.write().await;
        // Check again
        if let Some(session) = sessions.get(id) {
            return Ok(session.clone());
        }

        let session = Session::new_with_id(id.to_string());
        let session_arc = Arc::new(RwLock::new(session));
        sessions.insert(id.to_string(), session_arc.clone());

        info!("Created new session in memory: {}", id);
        Ok(session_arc)
    }

    fn start_auto_save_task(&self) {
        let sessions_map_arc = self.sessions.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60)); // Check every 60s

            loop {
                interval.tick().await;

                // Get all session handles
                let handles: Vec<(String, Arc<RwLock<Session>>)> = {
                    let map = sessions_map_arc.read().await;
                    map.iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect()
                };

                for (id, session_arc) in handles {
                    // Try to acquire read lock on session
                    // We use try_read to avoid blocking if session is busy being written to
                    // If busy, skip save this cycle
                    if let Ok(session) = session_arc.try_read() {
                        // In real impl, check dirty flag. For now, just call save (fs overhead only if writes happen)
                        if let Err(e) = session.save() {
                            warn!("Failed to auto-save session {}: {}", id, e);
                        }
                    }
                }
            }
        });
    }
}
