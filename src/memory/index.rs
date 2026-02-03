use anyhow::{anyhow, Result};
use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::debug;

use super::embeddings::{cosine_similarity, deserialize_embedding, serialize_embedding};
use super::search::MemoryChunk;

#[derive(Clone)]
pub struct MemoryIndex {
    conn: Arc<Mutex<Connection>>,
    workspace: PathBuf,
    db_path: PathBuf,
}

#[derive(Debug)]
pub struct ReindexStats {
    pub files_processed: usize,
    pub files_updated: usize,
    pub chunks_indexed: usize,
    pub duration: Duration,
}

impl MemoryIndex {
    /// Create a new memory index with database at the specified path
    pub fn new_with_db_path(workspace: &Path, db_path: &Path) -> Result<Self> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;

        // Initialize schema
        conn.execute_batch(
            r#"
            -- File tracking
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                hash TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                size INTEGER NOT NULL
            );

            -- Chunked content (embedding columns added via migration)
            CREATE TABLE IF NOT EXISTS chunks (
                id INTEGER PRIMARY KEY,
                file_path TEXT NOT NULL,
                line_start INTEGER NOT NULL,
                line_end INTEGER NOT NULL,
                content TEXT NOT NULL,
                FOREIGN KEY (file_path) REFERENCES files(path) ON DELETE CASCADE
            );

            -- Full-text search index
            CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
                content,
                content='chunks',
                content_rowid='id'
            );

            -- Triggers to keep FTS in sync
            CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
            END;

            CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
                INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES('delete', old.id, old.content);
                INSERT INTO chunks_fts(rowid, content) VALUES (new.id, new.content);
            END;
            "#,
        )?;

        // Migration: add embedding columns if they don't exist
        Self::migrate_add_embedding_columns(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            workspace: workspace.to_path_buf(),
            db_path: db_path.to_path_buf(),
        })
    }

    /// Create a new memory index with database in workspace (legacy path)
    pub fn new(workspace: &Path) -> Result<Self> {
        let db_path = workspace.join("memory.sqlite");
        Self::new_with_db_path(workspace, &db_path)
    }

    /// Index a file, returning true if it was updated
    pub fn index_file(&self, path: &Path, force: bool) -> Result<bool> {
        let content = fs::read_to_string(path)?;
        let hash = hash_content(&content);
        let metadata = fs::metadata(path)?;
        let mtime = metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        let size = metadata.len() as i64;

        let relative_path = path
            .strip_prefix(&self.workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        // Check if file has changed
        if !force {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT hash FROM files WHERE path = ?1",
                    params![&relative_path],
                    |row| row.get(0),
                )
                .ok();

            if existing.as_deref() == Some(&hash) {
                debug!("File unchanged, skipping: {}", relative_path);
                return Ok(false);
            }
        }

        debug!("Indexing file: {}", relative_path);

        // Update file record
        conn.execute(
            "INSERT OR REPLACE INTO files (path, hash, mtime, size) VALUES (?1, ?2, ?3, ?4)",
            params![&relative_path, &hash, mtime, size],
        )?;

        // Delete existing chunks
        conn.execute(
            "DELETE FROM chunks WHERE file_path = ?1",
            params![&relative_path],
        )?;

        // Create new chunks
        let chunks = chunk_text(&content, 400, 80);

        for (_i, chunk) in chunks.iter().enumerate() {
            conn.execute(
                "INSERT INTO chunks (file_path, line_start, line_end, content) VALUES (?1, ?2, ?3, ?4)",
                params![&relative_path, chunk.line_start, chunk.line_end, &chunk.content],
            )?;
        }

        Ok(true)
    }

    /// Search using FTS5
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryChunk>> {
        // Escape special FTS5 characters
        let escaped_query = escape_fts_query(query);

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        let mut stmt = conn.prepare(
            r#"
            SELECT c.file_path, c.line_start, c.line_end, c.content, bm25(chunks_fts) as score
            FROM chunks_fts fts
            JOIN chunks c ON fts.rowid = c.id
            WHERE chunks_fts MATCH ?1
            ORDER BY score
            LIMIT ?2
            "#,
        )?;

        let rows = stmt.query_map(params![&escaped_query, limit as i64], |row| {
            Ok(MemoryChunk {
                file: row.get(0)?,
                line_start: row.get(1)?,
                line_end: row.get(2)?,
                content: row.get(3)?,
                score: row.get::<_, f64>(4)?.abs(), // BM25 returns negative scores
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        Ok(results)
    }

    /// Get total chunk count
    pub fn chunk_count(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get chunk count for a specific file
    pub fn file_chunk_count(&self, path: &Path) -> Result<usize> {
        let relative_path = path
            .strip_prefix(&self.workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE file_path = ?1",
            params![&relative_path],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get database size in bytes
    pub fn size_bytes(&self) -> Result<u64> {
        if self.db_path.exists() {
            Ok(fs::metadata(&self.db_path)?.len())
        } else {
            Ok(0)
        }
    }

    /// Get the database path
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Migration: add embedding columns if they don't exist
    fn migrate_add_embedding_columns(conn: &Connection) -> Result<()> {
        // Check if embedding column exists
        let has_embedding: bool = conn
            .prepare("SELECT embedding FROM chunks LIMIT 1")
            .is_ok();

        if !has_embedding {
            debug!("Migrating: adding embedding columns to chunks");
            conn.execute("ALTER TABLE chunks ADD COLUMN embedding TEXT", [])?;
            conn.execute("ALTER TABLE chunks ADD COLUMN embedding_model TEXT", [])?;
        }

        // Create index (safe to run even if exists due to IF NOT EXISTS)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_chunks_embedding ON chunks(embedding_model)",
            [],
        )?;

        Ok(())
    }

    /// Get chunks that need embeddings
    pub fn chunks_without_embeddings(&self, limit: usize) -> Result<Vec<(i64, String)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        let mut stmt = conn.prepare(
            "SELECT id, content FROM chunks WHERE embedding IS NULL LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }

        Ok(results)
    }

    /// Store embedding for a chunk
    pub fn store_embedding(&self, chunk_id: i64, embedding: &[f32], model: &str) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        let embedding_json = serialize_embedding(embedding);

        conn.execute(
            "UPDATE chunks SET embedding = ?1, embedding_model = ?2 WHERE id = ?3",
            params![&embedding_json, model, chunk_id],
        )?;

        Ok(())
    }

    /// Vector search using embeddings
    pub fn search_vector(
        &self,
        query_embedding: &[f32],
        model: &str,
        limit: usize,
    ) -> Result<Vec<MemoryChunk>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        // Get all chunks with embeddings for this model
        let mut stmt = conn.prepare(
            "SELECT id, file_path, line_start, line_end, content, embedding
             FROM chunks
             WHERE embedding IS NOT NULL AND embedding_model = ?1",
        )?;

        let rows = stmt.query_map(params![model], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        // Compute similarities and sort
        let mut scored: Vec<(f32, MemoryChunk)> = Vec::new();

        for row in rows {
            let (_, file_path, line_start, line_end, content, embedding_json) = row?;
            let embedding = deserialize_embedding(&embedding_json);

            if embedding.len() == query_embedding.len() {
                let similarity = cosine_similarity(query_embedding, &embedding);
                scored.push((
                    similarity,
                    MemoryChunk {
                        file: file_path,
                        line_start,
                        line_end,
                        content,
                        score: similarity as f64,
                    },
                ));
            }
        }

        // Sort by similarity (descending)
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Take top results
        Ok(scored.into_iter().take(limit).map(|(_, chunk)| chunk).collect())
    }

    /// Hybrid search: combine FTS and vector results
    pub fn search_hybrid(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        model: &str,
        limit: usize,
        text_weight: f32,
        vector_weight: f32,
    ) -> Result<Vec<MemoryChunk>> {
        // Get FTS results
        let fts_results = self.search(query, limit * 2)?;

        // Get vector results if embedding provided
        let vector_results = if let Some(embedding) = query_embedding {
            self.search_vector(embedding, model, limit * 2)?
        } else {
            Vec::new()
        };

        // Merge results using weighted scores
        let mut merged: std::collections::HashMap<String, (f32, MemoryChunk)> =
            std::collections::HashMap::new();

        // Add FTS results (normalize BM25 score to 0-1 range)
        let max_fts_score = fts_results
            .iter()
            .map(|r| r.score)
            .fold(0.0f64, |a, b| a.max(b));
        let max_fts_score = if max_fts_score > 0.0 { max_fts_score } else { 1.0 };

        for result in fts_results {
            let key = format!("{}:{}:{}", result.file, result.line_start, result.line_end);
            let normalized_score = (result.score / max_fts_score) as f32;
            let weighted_score = normalized_score * text_weight;
            merged.insert(key, (weighted_score, result));
        }

        // Add/merge vector results
        for result in vector_results {
            let key = format!("{}:{}:{}", result.file, result.line_start, result.line_end);
            let weighted_score = result.score as f32 * vector_weight;

            if let Some((existing_score, existing_chunk)) = merged.get_mut(&key) {
                *existing_score += weighted_score;
                existing_chunk.score = *existing_score as f64;
            } else {
                let mut chunk = result;
                chunk.score = weighted_score as f64;
                merged.insert(key, (weighted_score, chunk));
            }
        }

        // Sort by combined score and take top results
        let mut results: Vec<_> = merged.into_values().collect();
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        Ok(results.into_iter().take(limit).map(|(_, chunk)| chunk).collect())
    }

    /// Count chunks with embeddings
    pub fn embedded_chunk_count(&self, model: &str) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow!("Lock poisoned: {}", e))?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL AND embedding_model = ?1",
            params![model],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }
}

fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn escape_fts_query(query: &str) -> String {
    // Wrap in quotes to treat as phrase, escape internal quotes
    let escaped = query.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

struct ChunkInfo {
    line_start: i32,
    line_end: i32,
    content: String,
}

fn chunk_text(text: &str, target_tokens: usize, overlap_tokens: usize) -> Vec<ChunkInfo> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();

    if lines.is_empty() {
        return chunks;
    }

    // Rough estimate: 4 chars per token
    let target_chars = target_tokens * 4;
    let overlap_chars = overlap_tokens * 4;

    let mut start_line = 0;
    let mut current_chars = 0;
    let mut chunk_lines = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        chunk_lines.push(*line);
        current_chars += line.len() + 1; // +1 for newline

        if current_chars >= target_chars || i == lines.len() - 1 {
            // Create chunk
            chunks.push(ChunkInfo {
                line_start: (start_line + 1) as i32,
                line_end: (i + 1) as i32,
                content: chunk_lines.join("\n"),
            });

            // Calculate overlap for next chunk
            let mut overlap_len = 0;
            let mut overlap_start = chunk_lines.len();

            for (j, line) in chunk_lines.iter().enumerate().rev() {
                overlap_len += line.len() + 1;
                if overlap_len >= overlap_chars {
                    overlap_start = j;
                    break;
                }
            }

            // Prepare for next chunk
            if overlap_start < chunk_lines.len() {
                start_line = start_line + overlap_start;
                chunk_lines = chunk_lines[overlap_start..].to_vec();
                current_chars = chunk_lines.iter().map(|l| l.len() + 1).sum();
            } else {
                start_line = i + 1;
                chunk_lines.clear();
                current_chars = 0;
            }
        }
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_chunk_text() {
        let text = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let chunks = chunk_text(text, 10, 2); // Small chunks for testing

        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].line_start, 1);
    }

    #[test]
    fn test_memory_index() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let workspace = temp_dir.path();

        // Create a test file
        let test_file = workspace.join("test.md");
        fs::write(
            &test_file,
            "# Test\n\nThis is a test document.\n\nWith multiple lines.",
        )?;

        let index = MemoryIndex::new(workspace)?;
        index.index_file(&test_file, false)?;

        assert!(index.chunk_count()? > 0);

        let results = index.search("test document", 10)?;
        assert!(!results.is_empty());

        Ok(())
    }
}
