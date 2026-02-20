/// MemoryPilot v2.1 Database Engine — SQLite + FTS5.
/// Features: dedup, importance, TTL, bulk ops, export, auto-prompt.
use std::path::Path;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use chrono::Utc;

const DB_DIR: &str = ".MemoryPilot";
const DB_FILE: &str = "memory.db";
const PROMPT_FILE: &str = "GLOBAL_PROMPT.md";
const DEDUP_THRESHOLD: f64 = 0.85;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub kind: String,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub source: String,
    pub importance: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_at: Option<String>,
    pub access_count: i32,
}
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub memory: Memory,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub memory_count: i64,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open() -> Result<Self, String> {
        let dir = dirs::home_dir().ok_or("Cannot find home directory")?.join(DB_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create dir: {}", e))?;
        Self::open_at(&dir.join(DB_FILE))
    }

    pub fn open_at(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("SQLite open: {}", e))?;
        conn.execute_batch("
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA cache_size = -8000;
            PRAGMA foreign_keys = ON;
        ").map_err(|e| format!("Pragma: {}", e))?;
        let db = Self { conn };
        db.init_schema()?;
        db.upgrade_schema()?;
        let _ = db.backfill_embeddings();
        Ok(db)
    }
    fn init_schema(&self) -> Result<(), String> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS memories (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                kind TEXT NOT NULL DEFAULT 'fact',
                project TEXT,
                tags TEXT NOT NULL DEFAULT '[]',
                source TEXT NOT NULL DEFAULT 'cursor',
                importance INTEGER NOT NULL DEFAULT 3,
                expires_at TEXT,
                metadata TEXT,
                embedding BLOB,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_accessed_at TEXT,
                access_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS memory_links (
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                relation_type TEXT NOT NULL DEFAULT 'relates_to',
                created_at TEXT NOT NULL,
                PRIMARY KEY (source_id, target_id),
                FOREIGN KEY (source_id) REFERENCES memories(id) ON DELETE CASCADE,
                FOREIGN KEY (target_id) REFERENCES memories(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_links_source ON memory_links(source_id);
            CREATE INDEX IF NOT EXISTS idx_links_target ON memory_links(target_id);

            CREATE TABLE IF NOT EXISTS memory_entities (
                memory_id TEXT NOT NULL,
                entity_kind TEXT NOT NULL,
                entity_value TEXT NOT NULL,
                FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_entities_value ON memory_entities(entity_value);
            CREATE INDEX IF NOT EXISTS idx_entities_memory ON memory_entities(memory_id);
            CREATE INDEX IF NOT EXISTS idx_memories_project ON memories(project);
            CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind);
            CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_memories_expires ON memories(expires_at) WHERE expires_at IS NOT NULL;

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content, tags, kind, project,
                content_rowid='rowid',
                tokenize='unicode61 remove_diacritics 2'
            );

            CREATE TABLE IF NOT EXISTS projects (
                name TEXT PRIMARY KEY,
                path TEXT NOT NULL DEFAULT '',
                description TEXT,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS config (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        ").map_err(|e| format!("Schema: {}", e))
    }
    /// Upgrade schema for existing databases (add new columns if missing).
    fn upgrade_schema(&self) -> Result<(), String> {
        // Check if importance column exists
        let has_importance: bool = self.conn
            .prepare("SELECT importance FROM memories LIMIT 0")
            .is_ok();
        if !has_importance {
            let _ = self.conn.execute_batch(
                "ALTER TABLE memories ADD COLUMN importance INTEGER NOT NULL DEFAULT 3;
                 ALTER TABLE memories ADD COLUMN expires_at TEXT;"
            );
        }
        // v3.0 columns
        let has_embedding: bool = self.conn
            .prepare("SELECT embedding FROM memories LIMIT 0")
            .is_ok();
        if !has_embedding {
            let _ = self.conn.execute_batch(
                "ALTER TABLE memories ADD COLUMN embedding BLOB;
                 ALTER TABLE memories ADD COLUMN last_accessed_at TEXT;
                 ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;
                 CREATE TABLE IF NOT EXISTS memory_links (
                     source_id TEXT NOT NULL,
                     target_id TEXT NOT NULL,
                     relation_type TEXT NOT NULL DEFAULT 'relates_to',
                     created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                     PRIMARY KEY (source_id, target_id),
                     FOREIGN KEY (source_id) REFERENCES memories(id) ON DELETE CASCADE,
                     FOREIGN KEY (target_id) REFERENCES memories(id) ON DELETE CASCADE
                 );
                 CREATE INDEX IF NOT EXISTS idx_links_source ON memory_links(source_id);
                 CREATE INDEX IF NOT EXISTS idx_links_target ON memory_links(target_id);

                 CREATE TABLE IF NOT EXISTS memory_entities (
                     memory_id TEXT NOT NULL,
                     entity_kind TEXT NOT NULL,
                     entity_value TEXT NOT NULL,
                     FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
                 );
                 CREATE INDEX IF NOT EXISTS idx_entities_value ON memory_entities(entity_value);
                 CREATE INDEX IF NOT EXISTS idx_entities_memory ON memory_entities(memory_id);"
            );
        }
        Ok(())
    }

    // ─── DEDUP ────────────────────────────────────────

    /// Normalize text for comparison: lowercase, collapse whitespace, strip punctuation.
    fn normalize(text: &str) -> String {
        text.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Jaccard similarity between two normalized strings (word-level).
    fn similarity(a: &str, b: &str) -> f64 {
        let a_words: std::collections::HashSet<&str> = a.split_whitespace().collect();
        let b_words: std::collections::HashSet<&str> = b.split_whitespace().collect();
        if a_words.is_empty() && b_words.is_empty() { return 1.0; }
        let intersection = a_words.intersection(&b_words).count() as f64;
        let union = a_words.union(&b_words).count() as f64;
        if union == 0.0 { 0.0 } else { intersection / union }
    }
    /// Find a near-duplicate in the same project/scope.
    fn find_duplicate(&self, content: &str, project: Option<&str>) -> Result<Option<Memory>, String> {
        let norm = Self::normalize(content);
        let memories: Vec<Memory> = if let Some(p) = project {
            let mut stmt = self.conn.prepare(
                "SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count FROM memories WHERE project=?1 ORDER BY updated_at DESC LIMIT 200"
            ).map_err(|e| format!("Dedup: {}", e))?;
            let rows = stmt.query_map(params![p], |r| Ok(row_to_memory(r)))
                .map_err(|e| format!("Dedup: {}", e))?;
            let collected: Vec<Memory> = rows.flatten().collect();
            collected
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count FROM memories WHERE project IS NULL ORDER BY updated_at DESC LIMIT 200"
            ).map_err(|e| format!("Dedup: {}", e))?;
            let rows = stmt.query_map([], |r| Ok(row_to_memory(r)))
                .map_err(|e| format!("Dedup: {}", e))?;
            let collected: Vec<Memory> = rows.flatten().collect();
            collected
        };
        for mem in memories {
            let mem_norm = Self::normalize(&mem.content);
            if Self::similarity(&norm, &mem_norm) >= DEDUP_THRESHOLD {
                return Ok(Some(mem));
            }
        }
        Ok(None)
    }
    // ─── KNOWLEDGE GRAPH ──────────────────────────────
    
    pub fn rebuild_links(&self, memory: &Memory) -> Result<(), String> {
        let entities = crate::graph::extract_entities(&memory.content, memory.project.as_deref());
        
        // 1. Update entities table
        let _ = self.conn.execute("DELETE FROM memory_entities WHERE memory_id = ?1", params![memory.id]);
        for entity in &entities {
            let _ = self.conn.execute(
                "INSERT OR IGNORE INTO memory_entities (memory_id, entity_kind, entity_value) VALUES (?1, ?2, ?3)",
                params![memory.id, entity.kind, entity.value],
            );
        }
        
        // 2. Find related memories via shared entities
        let mut target_ids = std::collections::HashSet::new();
        for entity in &entities {
            if let Ok(mut stmt) = self.conn.prepare("SELECT DISTINCT m.id, m.kind FROM memory_entities e JOIN memories m ON e.memory_id = m.id WHERE e.entity_value = ?1 AND e.memory_id != ?2 LIMIT 10") {
                if let Ok(rows) = stmt.query_map(params![entity.value, memory.id], |row| Ok((row.get::<_,String>(0)?, row.get::<_,String>(1)?))) {
                    for r in rows.flatten() { target_ids.insert((r.0, r.1)); }
                }
            }
        }
        
        let _ = self.conn.execute("DELETE FROM memory_links WHERE source_id = ?1 OR target_id = ?1", params![memory.id]);
        
        for (target_id, target_kind) in target_ids {
            let rel = crate::graph::infer_relation(&memory.kind, &target_kind);
            let _ = self.conn.execute(
                "INSERT OR IGNORE INTO memory_links (source_id, target_id, relation_type) VALUES (?1, ?2, ?3)",
                params![memory.id, target_id, rel]
            );
            let rev_rel = crate::graph::infer_relation(&target_kind, &memory.kind);
            let _ = self.conn.execute(
                "INSERT OR IGNORE INTO memory_links (source_id, target_id, relation_type) VALUES (?1, ?2, ?3)",
                params![target_id, memory.id, rev_rel]
            );
        }
        Ok(())
    }

    // ─── CRUD ────────────────────────────────────────

    /// Add memory with dedup check. Returns (memory, was_merged).
    pub fn add_memory(&self, content: &str, kind: &str, project: Option<&str>,
                      tags: &[String], source: &str, importance: i32,
                      expires_at: Option<&str>,
                      metadata: Option<&serde_json::Value>) -> Result<(Memory, bool), String> {
        // Check for near-duplicate
        if let Some(existing) = self.find_duplicate(content, project)? {
            // Merge: update content if newer is longer, bump updated_at
            let new_content = if content.len() > existing.content.len() { content } else { &existing.content };
            let new_importance = importance.max(existing.importance);
            let mut merged_tags: Vec<String> = existing.tags.clone();
            for t in tags { if !merged_tags.contains(t) { merged_tags.push(t.clone()); } }
            let updated = self.update_memory_full(&existing.id, Some(new_content), None,
                Some(&merged_tags), Some(new_importance), expires_at)?;
            return Ok((updated.unwrap_or(existing), true));
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());
        let meta_json = metadata.map(|m| serde_json::to_string(m).unwrap_or_default());
        let imp = importance.clamp(1, 5);
        let emb = crate::embedding::embed_text(content);
        let emb_blob = crate::embedding::vec_to_blob(&emb);

        self.conn.execute(
            "INSERT INTO memories (id,content,kind,project,tags,source,importance,expires_at,metadata,embedding,created_at,updated_at,access_count)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,0)",
            params![id, content, kind, project, tags_json, source, imp, expires_at, meta_json, emb_blob, now, now],
        ).map_err(|e| format!("Insert: {}", e))?;

        // FTS index
        let rowid = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO memories_fts (rowid,content,tags,kind,project) VALUES (?1,?2,?3,?4,?5)",
            params![rowid, content, tags_json, kind, project.unwrap_or("")],
        ).map_err(|e| format!("FTS insert: {}", e))?;

        if let Some(proj) = project { let _ = self.ensure_project(proj); }

        let mem = Memory { id, content: content.into(), kind: kind.into(), project: project.map(String::from),
            tags: tags.to_vec(), source: source.into(), importance: imp, expires_at: expires_at.map(String::from),
            created_at: now.clone(), updated_at: now, metadata: metadata.cloned(), last_accessed_at: None, access_count: 0 };
        let _ = self.rebuild_links(&mem);
        Ok((mem, false))
    }
    /// Full update with all fields.
    pub fn update_memory_full(&self, id: &str, content: Option<&str>, kind: Option<&str>,
                              tags: Option<&[String]>, importance: Option<i32>,
                              expires_at: Option<&str>) -> Result<Option<Memory>, String> {
        let existing = match self.get_memory(id)? { Some(m) => m, None => return Ok(None) };
        let now = Utc::now().to_rfc3339();
        let new_content = content.unwrap_or(&existing.content);
        let new_kind = kind.unwrap_or(&existing.kind);
        let new_tags = tags.map(|t| t.to_vec()).unwrap_or_else(|| existing.tags.clone());
        let tags_json = serde_json::to_string(&new_tags).unwrap_or_else(|_| "[]".into());
        let new_imp = importance.unwrap_or(existing.importance).clamp(1, 5);
        let new_exp = if expires_at.is_some() { expires_at.map(String::from) } else { existing.expires_at.clone() };
        let emb = crate::embedding::embed_text(new_content);
        let emb_blob = crate::embedding::vec_to_blob(&emb);

        self.conn.execute(
            "UPDATE memories SET content=?1,kind=?2,tags=?3,importance=?4,expires_at=?5,updated_at=?6,embedding=?7 WHERE id=?8",
            params![new_content, new_kind, tags_json, new_imp, new_exp, now, emb_blob, id],
        ).map_err(|e| format!("Update: {}", e))?;

        // Rebuild FTS
        if let Ok(rowid) = self.conn.query_row::<i64, _, _>(
            "SELECT rowid FROM memories WHERE id=?1", params![id], |r| r.get(0)) {
            let _ = self.conn.execute("DELETE FROM memories_fts WHERE rowid=?1", params![rowid]);
            let proj = existing.project.as_deref().unwrap_or("");
            let _ = self.conn.execute(
                "INSERT INTO memories_fts (rowid,content,tags,kind,project) VALUES (?1,?2,?3,?4,?5)",
                params![rowid, new_content, tags_json, new_kind, proj]);
        }

        let mem = Memory { id: id.into(), content: new_content.into(), kind: new_kind.into(),
            project: existing.project, tags: new_tags, source: existing.source,
            importance: new_imp, expires_at: new_exp,
            created_at: existing.created_at, updated_at: now, metadata: existing.metadata, 
            last_accessed_at: existing.last_accessed_at, access_count: existing.access_count };
        let _ = self.rebuild_links(&mem);
        Ok(Some(mem))
    }



    pub fn delete_memory(&self, id: &str) -> Result<bool, String> {
        if let Ok(rowid) = self.conn.query_row::<i64, _, _>(
            "SELECT rowid FROM memories WHERE id=?1", params![id], |r| r.get(0)) {
            let _ = self.conn.execute("DELETE FROM memories_fts WHERE rowid=?1", params![rowid]);
        }
        let affected = self.conn.execute("DELETE FROM memories WHERE id=?1", params![id])
            .map_err(|e| format!("Delete: {}", e))?;
        Ok(affected > 0)
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count FROM memories WHERE id=?1"
        ).map_err(|e| format!("Prepare: {}", e))?;
        let mut rows = stmt.query(params![id]).map_err(|e| format!("Query: {}", e))?;
        match rows.next().map_err(|e| format!("Next: {}", e))? {
            Some(row) => Ok(Some(row_to_memory(row))),
            None => Ok(None),
        }
    }

    // ─── BULK ADD ─────────────────────────────────────

    /// Add multiple memories in one call, with dedup per item. Returns (added, merged, skipped).
    pub fn add_memories_bulk(&self, items: &[BulkItem]) -> Result<(Vec<Memory>, usize, usize), String> {
        let mut added: Vec<Memory> = Vec::new();
        let mut merged = 0usize;
        let mut skipped = 0usize;
        for item in items {
            if item.content.trim().is_empty() { skipped += 1; continue; }
            let tags: Vec<String> = item.tags.clone().unwrap_or_default();
            let imp = item.importance.unwrap_or(3);
            let exp = item.expires_at.as_deref();
            match self.add_memory(&item.content, &item.kind, item.project.as_deref(),
                                  &tags, &item.source, imp, exp, None) {
                Ok((mem, was_merged)) => {
                    if was_merged { merged += 1; } else { added.push(mem); }
                }
                Err(_) => { skipped += 1; }
            }
        }
        Ok((added, merged, skipped))
    }
    // ─── SEARCH (FTS5 BM25 × importance) ──────────────

    pub fn search(&self, query: &str, limit: usize, project: Option<&str>,
                  kind: Option<&str>, tags: Option<&[String]>, watcher_keywords: Option<&[String]>) -> Result<Vec<SearchResult>, String> {
        let fts_terms: String = query.split_whitespace()
            .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        if fts_terms.is_empty() { return Ok(Vec::new()); }

        // Clean expired before search
        let _ = self.cleanup_expired();

        let query_emb = crate::embedding::embed_text(query);

        // 1. BM25 Search
        let mut conditions = vec!["memories_fts MATCH ?1".to_string()];
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(fts_terms.clone())];

        if let Some(p) = project {
            conditions.push(format!("m.project = ?{}", param_values.len() + 1));
            param_values.push(Box::new(p.to_string()));
        }
        if let Some(k) = kind {
            conditions.push(format!("m.kind = ?{}", param_values.len() + 1));
            param_values.push(Box::new(k.to_string()));
        }

        let where_clause = conditions.join(" AND ");
        let sql = format!(
            "SELECT m.id,m.content,m.kind,m.project,m.tags,m.source,m.importance,m.expires_at,m.metadata,m.created_at,m.updated_at,m.last_accessed_at,m.access_count,
                    bm25(memories_fts, 10.0, 3.0, 1.0, 2.0) AS bm25_score
             FROM memories_fts f
             JOIN memories m ON m.rowid = f.rowid
             WHERE {}
             ORDER BY bm25_score ASC
             LIMIT 100", where_clause);
             
        let mut stmt = self.conn.prepare(&sql).map_err(|e| format!("Search prepare: {}", e))?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let mut bm25_results = std::collections::HashMap::new();
        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let mem = row_to_memory(row);
            let bm25: f64 = row.get(13)?;
            Ok((mem, bm25))
        }).map_err(|e| format!("Search: {}", e))?;
        
        let mut rank = 1;
        let mut all_memories = std::collections::HashMap::new();
        for r in rows.flatten() {
            let (mem, _) = r;
            bm25_results.insert(mem.id.clone(), rank);
            all_memories.insert(mem.id.clone(), mem);
            rank += 1;
        }

        // 2. Vector Search (Fetch embeddings matching filters)
        let mut vec_conditions = Vec::new();
        let mut vec_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(p) = project {
            vec_conditions.push(format!("project = ?{}", vec_params.len() + 1));
            vec_params.push(Box::new(p.to_string()));
        }
        if let Some(k) = kind {
            vec_conditions.push(format!("kind = ?{}", vec_params.len() + 1));
            vec_params.push(Box::new(k.to_string()));
        }
        let vec_where = if vec_conditions.is_empty() { String::new() } else { format!("WHERE {}", vec_conditions.join(" AND ")) };
        let vec_sql = format!("SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count,embedding FROM memories {}", vec_where);
        let mut stmt2 = self.conn.prepare(&vec_sql).map_err(|e| format!("Vector Search: {}", e))?;
        let vec_refs: Vec<&dyn rusqlite::types::ToSql> = vec_params.iter().map(|p| p.as_ref()).collect();
        
        let mut vector_scores: Vec<(String, f32)> = Vec::new();
        let rows2 = stmt2.query_map(vec_refs.as_slice(), |row| {
            let mem = row_to_memory(row);
            let blob: Option<Vec<u8>> = row.get(13)?;
            Ok((mem, blob))
        }).map_err(|e| format!("Vector Search error: {}", e))?;
        
        for r in rows2.flatten() {
            let (mem, blob) = r;
            all_memories.entry(mem.id.clone()).or_insert_with(|| mem.clone());
            if let Some(b) = blob {
                let emb = crate::embedding::blob_to_vec(&b);
                let score = crate::embedding::cosine_similarity(&query_emb, &emb);
                vector_scores.push((mem.id, score));
            } else {
                vector_scores.push((mem.id, 0.0));
            }
        }
        
        // Sort vector scores descending
        vector_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut vector_results = std::collections::HashMap::new();
        for (i, (id, _)) in vector_scores.iter().take(100).enumerate() {
            vector_results.insert(id.clone(), i + 1);
        }

        // 3. RRF Fusion
        let mut rrf_scores: Vec<(String, f64)> = Vec::new();
        
        // Fetch graph links for PageRank-like boost
        let mut link_boosts: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT target_id, relation_type FROM memory_links") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?))) {
                for r in rows.flatten() {
                    let (target, relation) = r;
                    let boost = match relation.as_str() {
                        "deprecates" => -0.9, // heavy penalty
                        "depends_on" | "implements" | "resolves" => 0.1, // incoming link boost
                        _ => 0.05,
                    };
                    *link_boosts.entry(target).or_default() += boost;
                }
            }
        }
        
        for (id, mem) in &all_memories {
            let bm25_rank = bm25_results.get(id).copied().unwrap_or(1000);
            let vec_rank = vector_results.get(id).copied().unwrap_or(1000);
            let mut score = crate::embedding::rrf_score(bm25_rank, vec_rank);
            
            // Boost score by importance (1.0 to 5.0 factor approx)
            score = score * (mem.importance as f64 / 3.0); 
            
            // PageRank-like link boost
            if let Some(lb) = link_boosts.get(id) {
                if *lb < 0.0 {
                    score *= 1.0 + lb; // penalty (e.g. 1.0 - 0.9 = 0.1x score)
                } else {
                    score *= 1.0 + lb; // boost
                }
            }
            
            // Watcher boost (dynamic context)
            if let Some(keywords) = watcher_keywords {
                let content_lower = mem.content.to_lowercase();
                let match_count = keywords.iter().filter(|w| content_lower.contains(w.to_lowercase().as_str())).count();
                if match_count > 0 {
                    score *= 1.0 + (match_count as f64 * 0.2); // +20% per matching keyword
                }
            }
            
            // Also boost if tag match
            if let Some(filter_tags) = tags {
                let filter_set: std::collections::HashSet<String> = filter_tags.iter().map(|t| t.to_lowercase()).collect();
                if mem.tags.iter().any(|t| filter_set.contains(&t.to_lowercase())) {
                    score *= 1.5;
                } else {
                    score *= 0.1; // penalize if tags are requested but don't match
                }
            }
            rrf_scores.push((id.clone(), score));
        }

        rrf_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut results: Vec<SearchResult> = Vec::new();
        for (id, score) in rrf_scores.into_iter().take(limit) {
            if let Some(mem) = all_memories.remove(&id) {
                results.push(SearchResult { memory: mem, score: (score * 10000.0).round() / 10000.0 });
            }
        }
        
        // Update access count and timestamp for returned results
        for res in &results {
            let _ = self.conn.execute("UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2", 
                params![chrono::Utc::now().to_rfc3339(), res.memory.id]);
        }

        Ok(results)
    }
    // ─── LIST ─────────────────────────────────────────

    pub fn list_memories(&self, project: Option<&str>, kind: Option<&str>,
                         limit: usize, offset: usize) -> Result<(Vec<Memory>, i64), String> {
        let _ = self.cleanup_expired();

        let mut conditions: Vec<String> = Vec::new();
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(p) = project {
            conditions.push(format!("project = ?{}", param_values.len() + 1));
            param_values.push(Box::new(p.to_string()));
        }
        if let Some(k) = kind {
            conditions.push(format!("kind = ?{}", param_values.len() + 1));
            param_values.push(Box::new(k.to_string()));
        }

        let where_clause = if conditions.is_empty() { String::new() }
            else { format!(" WHERE {}", conditions.join(" AND ")) };

        let count_sql = format!("SELECT COUNT(*) FROM memories{}", where_clause);
        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();
        let total: i64 = self.conn.query_row(&count_sql, param_refs.as_slice(), |r| r.get(0))
            .map_err(|e| format!("Count: {}", e))?;

        let data_sql = format!(
            "SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count FROM memories{} ORDER BY updated_at DESC LIMIT ?{} OFFSET ?{}",
            where_clause, param_values.len() + 1, param_values.len() + 2);
        param_values.push(Box::new(limit as i64));
        param_values.push(Box::new(offset as i64));
        let param_refs2: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let mut stmt = self.conn.prepare(&data_sql).map_err(|e| format!("List: {}", e))?;
        let memories: Vec<Memory> = stmt.query_map(param_refs2.as_slice(), |r| Ok(row_to_memory(r)))
            .map_err(|e| format!("List query: {}", e))?
            .filter_map(|r| r.ok())
            .collect();
        Ok((memories, total))
    }
    // ─── TTL / EXPIRATION ─────────────────────────────

    pub fn cleanup_expired(&self) -> Result<usize, String> {
        let now = Utc::now().to_rfc3339();
        // Delete FTS entries first
        let _ = self.conn.execute(
            "DELETE FROM memories_fts WHERE rowid IN (SELECT rowid FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?1)",
            params![now]);
        let affected = self.conn.execute(
            "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?1", params![now]
        ).map_err(|e| format!("Cleanup: {}", e))?;
        Ok(affected)
    }

    // ─── GC & COMPRESSION ─────────────────────────────
    
    pub fn run_gc(&self, config: &crate::gc::GcConfig, dry_run: bool) -> Result<crate::gc::GcReport, String> {
        let db_path = dirs::home_dir().unwrap_or_default().join(DB_DIR).join(DB_FILE);
        let size_before = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        
        let mut expired_removed = 0;
        if !dry_run {
            expired_removed = self.cleanup_expired()?;
        }
        
        // Find mergeable candidates
        let now = chrono::Utc::now();
        let mut groups_merged = 0;
        let mut memories_compressed = 0;
        
        for kind in &config.compressible_kinds {
            let sql = format!(
                "SELECT id, content, project, importance, updated_at FROM memories WHERE kind = ?1"
            );
            if let Ok(mut stmt) = self.conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map(params![kind], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i32>(3)?, r.get::<_, String>(4)?))
                }) {
                    let mut by_project: std::collections::HashMap<Option<String>, Vec<(String, String)>> = std::collections::HashMap::new();
                    for row in rows.flatten() {
                        let updated_at = chrono::DateTime::parse_from_rfc3339(&row.4).unwrap_or_else(|_| chrono::Utc::now().into());
                        let age_days = (now - updated_at.with_timezone(&chrono::Utc)).num_days();
                        
                        let score = crate::gc::gc_score(row.3, age_days, kind, config);
                        if score > 0.6 && row.3 < config.importance_threshold && age_days >= config.age_days {
                            by_project.entry(row.2).or_default().push((row.0, row.1));
                        }
                    }
                    
                    for (proj, mut items) in by_project {
                        if items.len() > 1 {
                            items.truncate(config.max_merge_group);
                            let contents: Vec<String> = items.iter().map(|i| i.1.clone()).collect();
                            let merged_content = crate::gc::merge_memories(&contents, kind, proj.as_deref());
                            
                            let ids_to_delete: Vec<String> = items.iter().map(|i| i.0.clone()).collect();
                            
                            if !dry_run {
                                if self.add_memory(&merged_content, kind, proj.as_deref(), &["merged".to_string()], "gc_compressor", 3, None, None).is_ok() {
                                    for id in ids_to_delete {
                                        let _ = self.delete_memory(&id);
                                        memories_compressed += 1;
                                    }
                                    groups_merged += 1;
                                }
                            } else {
                                memories_compressed += ids_to_delete.len();
                                groups_merged += 1;
                            }
                        }
                    }
                }
            }
        }
        
        let mut orphan_links_removed = 0;
        if !dry_run {
            orphan_links_removed += self.conn.execute(
                "DELETE FROM memory_entities WHERE memory_id NOT IN (SELECT id FROM memories)",
                []
            ).unwrap_or(0);
            
            orphan_links_removed += self.conn.execute(
                "DELETE FROM memory_links WHERE source_id NOT IN (SELECT id FROM memories) OR target_id NOT IN (SELECT id FROM memories)",
                []
            ).unwrap_or(0);
        }
        
        let size_after = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        
        Ok(crate::gc::GcReport {
            expired_removed,
            groups_merged,
            memories_compressed,
            orphan_links_removed: orphan_links_removed as usize,
            db_size_before: size_before,
            db_size_after: size_after,
        })
    }

    // ─── EXPORT ───────────────────────────────────────

    pub fn export_memories(&self, project: Option<&str>, format: &str) -> Result<String, String> {
        let (memories, _) = self.list_memories(project, None, 10000, 0)?;
        match format {
            "json" => serde_json::to_string_pretty(&memories).map_err(|e| format!("JSON: {}", e)),
            "markdown" | "md" => {
                let mut md = String::new();
                let title = project.unwrap_or("All Memories");
                md.push_str(&format!("# MemoryPilot Export: {}\n\n", title));
                md.push_str(&format!("Total: {} memories\n\n", memories.len()));

                let mut by_kind: std::collections::BTreeMap<String, Vec<&Memory>> = std::collections::BTreeMap::new();
                for m in &memories { by_kind.entry(m.kind.clone()).or_default().push(m); }

                for (kind, mems) in &by_kind {
                    md.push_str(&format!("## {} ({})\n\n", kind, mems.len()));
                    for m in mems {
                        let tags = if m.tags.is_empty() { String::new() }
                            else { format!(" `{}`", m.tags.join("` `")) };
                        let imp = "★".repeat(m.importance as usize);
                        md.push_str(&format!("- [{}] {}{}\n", imp, m.content, tags));
                    }
                    md.push('\n');
                }
                Ok(md)
            }
            _ => Err(format!("Unknown format '{}'. Use 'json' or 'markdown'.", format)),
        }
    }
    // ─── PROJECTS ─────────────────────────────────────

    fn ensure_project(&self, name: &str) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute("INSERT OR IGNORE INTO projects (name,path,created_at) VALUES (?1,'',?2)", params![name, now])
            .map_err(|e| format!("Ensure: {}", e))?;
        Ok(())
    }

    pub fn register_project(&self, name: &str, path: &str, description: Option<&str>) -> Result<Project, String> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO projects (name,path,description,created_at) VALUES (?1,?2,?3,?4)
             ON CONFLICT(name) DO UPDATE SET path=?2, description=COALESCE(?3,description)",
            params![name, path, description, now],
        ).map_err(|e| format!("Register: {}", e))?;
        let count: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories WHERE project=?1", params![name], |r| r.get(0)).unwrap_or(0);
        Ok(Project { name: name.into(), path: path.into(), description: description.map(String::from), created_at: now, memory_count: count })
    }

    pub fn list_projects(&self) -> Result<Vec<Project>, String> {
        let mut stmt = self.conn.prepare(
            "SELECT p.name, p.path, p.description, p.created_at, COUNT(m.id) as cnt
             FROM projects p LEFT JOIN memories m ON m.project = p.name
             GROUP BY p.name ORDER BY cnt DESC"
        ).map_err(|e| format!("List projects: {}", e))?;
        let projects = stmt.query_map([], |row| {
            Ok(Project { name: row.get(0)?, path: row.get(1)?, description: row.get(2)?,
                created_at: row.get(3)?, memory_count: row.get(4)? })
        }).map_err(|e| format!("Projects: {}", e))?.filter_map(|r| r.ok()).collect();
        Ok(projects)
    }

    pub fn detect_project(&self, working_dir: &str) -> Result<Option<String>, String> {
        let mut stmt = self.conn.prepare("SELECT name, path FROM projects WHERE path != '' ORDER BY length(path) DESC")
            .map_err(|e| format!("Detect: {}", e))?;
        let projects: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| format!("Detect2: {}", e))?.filter_map(|r| r.ok()).collect();
        for (name, path) in &projects {
            if working_dir.starts_with(path) { return Ok(Some(name.clone())); }
        }
        let dir_name = std::path::Path::new(working_dir)
            .file_name().and_then(|n| n.to_str())
            .map(|n| n.to_lowercase().replace(|c: char| !c.is_alphanumeric() && c != '-', "-"));
        Ok(dir_name)
    }
    // ─── STATS ────────────────────────────────────────

    pub fn stats(&self) -> Result<serde_json::Value, String> {
        let total: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0)).unwrap_or(0);
        let global: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories WHERE project IS NULL", [], |r| r.get(0)).unwrap_or(0);
        let projects: i64 = self.conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap_or(0);
        let expired: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories WHERE expires_at IS NOT NULL AND expires_at < ?1",
            params![Utc::now().to_rfc3339()], |r| r.get(0)).unwrap_or(0);

        let mut by_kind = serde_json::Map::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT kind, COUNT(*) FROM memories GROUP BY kind") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?))) {
                for row in rows.flatten() { by_kind.insert(row.0, serde_json::json!(row.1)); }
            }
        }
        let mut by_project = serde_json::Map::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT COALESCE(project,'__global__'), COUNT(*) FROM memories GROUP BY project") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,i64>(1)?))) {
                for row in rows.flatten() { by_project.insert(row.0, serde_json::json!(row.1)); }
            }
        }
        let db_path = dirs::home_dir().unwrap_or_default().join(DB_DIR).join(DB_FILE);
        let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
        let size_str = if size < 1024 { format!("{} B", size) }
            else if size < 1048576 { format!("{} KB", size / 1024) }
            else { format!("{:.1} MB", size as f64 / 1048576.0) };

        Ok(serde_json::json!({ "total_memories": total, "global_memories": global, "projects": projects,
            "expired_pending": expired, "by_kind": by_kind, "by_project": by_project, "db_size": size_str }))
    }
    // ─── CONFIG ───────────────────────────────────────

    pub fn get_config(&self, key: &str) -> Option<String> {
        self.conn.query_row("SELECT value FROM config WHERE key=?1", params![key], |r| r.get(0)).ok()
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn.execute("INSERT INTO config (key,value) VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2",
            params![key, value]).map_err(|e| format!("Config: {}", e))?;
        Ok(())
    }

    // ─── GLOBAL PROMPT (auto-scan) ────────────────────

    pub fn get_global_prompt(&self, project: Option<&str>, working_dir: Option<&str>) -> Option<String> {
        let mut prompts: Vec<String> = Vec::new();

        // Helper to read file if modified since last cache, or use cache
        fn get_cached_prompt(path: &std::path::Path) -> Option<String> {
            if !path.exists() { return None; }
            let metadata = std::fs::metadata(path).ok()?;
            let modified = metadata.modified().ok()?;
            
            let mut cache = crate::PROMPT_CACHE.lock().unwrap();
            let path_str = path.to_string_lossy().to_string();
            
            if let Some((last_mod, content)) = cache.get(&path_str) {
                if last_mod == &modified {
                    return Some(content.clone());
                }
            }
            
            if let Ok(content) = std::fs::read_to_string(path) {
                cache.insert(path_str, (modified, content.clone()));
                Some(content)
            } else {
                None
            }
        }

        // 1. Check configured path
        if let Some(path_str) = self.get_config("global_prompt_path") {
            let path = std::path::Path::new(&path_str);
            if let Some(content) = get_cached_prompt(path) { prompts.push(content); }
        }

        // 2. Auto-scan ~/.MemoryPilot/GLOBAL_PROMPT.md
        let home_prompt = dirs::home_dir().map(|h| h.join(DB_DIR).join(PROMPT_FILE));
        if let Some(path) = &home_prompt {
            if let Some(content) = get_cached_prompt(path) {
                if !prompts.iter().any(|p| p == &content) { prompts.push(content); }
            }
        }

        // 3. Auto-scan project root GLOBAL_PROMPT.md
        let proj_dir: Option<String> = working_dir.map(String::from).or_else(|| {
            let proj_name = project?;
            let mut stmt = self.conn.prepare("SELECT path FROM projects WHERE name=?1").ok()?;
            stmt.query_row(params![proj_name], |r| r.get::<_,String>(0)).ok()
        });
        
        if let Some(dir) = proj_dir {
            let proj_prompt = std::path::Path::new(&dir).join(PROMPT_FILE);
            if let Some(content) = get_cached_prompt(&proj_prompt) {
                if !prompts.iter().any(|p| p == &content) { prompts.push(content); }
            }
        }

        if prompts.is_empty() { None } else { Some(prompts.join("\n\n---\n\n")) }
    }
    // ─── PROJECT CONTEXT ──────────────────────────────

    pub fn backfill_embeddings(&self) -> Result<usize, String> {
        let mut count = 0;
        let mut stmt = self.conn.prepare("SELECT id, content FROM memories WHERE embedding IS NULL")
            .map_err(|e| format!("Backfill prepare: {}", e))?;
        
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| format!("Backfill query: {}", e))?;
        
        let mut updates = Vec::new();
        for r in rows.flatten() {
            updates.push(r);
        }
        
        for (id, content) in updates {
            let emb = crate::embedding::embed_text(&content);
            let blob = crate::embedding::vec_to_blob(&emb);
            let _ = self.conn.execute(
                "UPDATE memories SET embedding = ?1 WHERE id = ?2",
                params![blob, id]
            );
            count += 1;
        }
        Ok(count)
    }

    pub fn get_project_brain(&self, project: &str, max_tokens: Option<usize>) -> Result<serde_json::Value, String> {
        let max_t = max_tokens.unwrap_or(1500);
        let max_chars = max_t * 4;
        let mut current_chars = 0;
        
        let mut tech_stack = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT DISTINCT entity_value FROM memory_entities e JOIN memories m ON e.memory_id = m.id WHERE m.project = ?1 AND e.entity_kind = 'tech' LIMIT 15") {
            if let Ok(rows) = stmt.query_map(params![project], |r| r.get::<_, String>(0)) {
                for tech in rows.flatten() {
                    let len = tech.len();
                    if current_chars + len > max_chars { break; }
                    current_chars += len;
                    tech_stack.push(tech);
                }
            }
        }
        
        let (core_arch, _) = self.list_memories(Some(project), Some("architecture"), 10, 0)?;
        let mut arch_content = Vec::new();
        for m in core_arch {
            if current_chars + m.content.len() > max_chars { break; }
            current_chars += m.content.len();
            arch_content.push(m.content);
        }

        let (decisions, _) = self.list_memories(Some(project), Some("decision"), 10, 0)?;
        let mut dec_content = Vec::new();
        for m in decisions {
            if current_chars + m.content.len() > max_chars { break; }
            current_chars += m.content.len();
            dec_content.push(m.content);
        }

        let (bugs, _) = self.list_memories(Some(project), Some("bug"), 10, 0)?;
        let mut bug_content = Vec::new();
        for m in bugs {
            if current_chars + m.content.len() > max_chars { break; }
            current_chars += m.content.len();
            bug_content.push(m.content);
        }
        
        let mut recent_content = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT content FROM memories WHERE project = ?1 AND updated_at > datetime('now','-7 days') ORDER BY updated_at DESC LIMIT 10") {
            if let Ok(rows) = stmt.query_map(params![project], |r| r.get::<_, String>(0)) {
                for content in rows.flatten() {
                    if current_chars + content.len() > max_chars { break; }
                    current_chars += content.len();
                    recent_content.push(content);
                }
            }
        }
        
        let mut key_components = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT DISTINCT entity_value FROM memory_entities e JOIN memories m ON e.memory_id = m.id WHERE m.project = ?1 AND e.entity_kind IN ('component', 'file') LIMIT 15") {
            if let Ok(rows) = stmt.query_map(params![project], |r| r.get::<_, String>(0)) {
                for comp in rows.flatten() {
                    let len = comp.len();
                    if current_chars + len > max_chars { break; }
                    current_chars += len;
                    key_components.push(comp);
                }
            }
        }

        Ok(serde_json::json!({
            "project": project,
            "tech_stack": tech_stack,
            "core_architecture": arch_content,
            "current_critical_decisions": dec_content,
            "active_bugs_known": bug_content,
            "recent_changes": recent_content,
            "key_components": key_components,
            "approx_tokens_used": current_chars / 4
        }))
    }

    pub fn get_project_context(&self, project: Option<&str>, working_dir: Option<&str>) -> Result<serde_json::Value, String> {
        let proj_name = match project {
            Some(p) => Some(p.to_string()),
            None => match working_dir { Some(wd) => self.detect_project(wd)?, None => None }
        };
        let proj_ref = proj_name.as_deref();
        let (proj_memories, proj_total) = if let Some(p) = proj_ref {
            self.list_memories(Some(p), None, 100, 0)?
        } else { (vec![], 0) };
        let (prefs, _) = self.list_memories(None, Some("preference"), 50, 0)?;
        let (patterns, _) = self.list_memories(None, Some("pattern"), 50, 0)?;
        let (snippets, _) = self.list_memories(None, Some("snippet"), 20, 0)?;

        Ok(serde_json::json!({
            "project": proj_ref.unwrap_or("none"),
            "project_memories": proj_total,
            "global_preferences": prefs.len(),
            "global_patterns": patterns.len(),
            "context": {
                "project": proj_memories.iter().map(|m| serde_json::json!({"kind":m.kind,"content":m.content,"tags":m.tags,"importance":m.importance})).collect::<Vec<_>>(),
                "preferences": prefs.iter().map(|m| &m.content).collect::<Vec<_>>(),
                "patterns": patterns.iter().map(|m| serde_json::json!({"content":m.content,"tags":m.tags})).collect::<Vec<_>>(),
                "snippets": snippets.iter().map(|m| serde_json::json!({"content":m.content,"tags":m.tags})).collect::<Vec<_>>(),
            }
        }))
    }
    // ─── RECALL (auto-context loader) ─────────────────

    /// One-shot context loader for new conversations.
    /// Combines: project context, global prompt, critical memories, and optional hint search.
    pub fn recall(&self, project: Option<&str>, working_dir: Option<&str>, hints: Option<&str>) -> Result<serde_json::Value, String> {
        // Auto-detect project
        let proj_name = match project {
            Some(p) => Some(p.to_string()),
            None => match working_dir { Some(wd) => self.detect_project(wd)?, None => None }
        };
        let proj_ref = proj_name.as_deref();

        // 1. Project memories (if project detected)
        let (proj_memories, proj_total) = if let Some(p) = proj_ref {
            self.list_memories(Some(p), None, 50, 0)?
        } else { (vec![], 0) };

        // 2. Global preferences + patterns (always useful)
        let (prefs, _) = self.list_memories(None, Some("preference"), 30, 0)?;
        let (patterns, _) = self.list_memories(None, Some("pattern"), 20, 0)?;
        let (decisions, _) = self.list_memories(None, Some("decision"), 20, 0)?;

        // 3. Critical memories (importance >= 4, any project)
        let critical: Vec<Memory> = {
            let mut stmt = self.conn.prepare(
                "SELECT id,content,kind,project,tags,source,importance,expires_at,metadata,created_at,updated_at,last_accessed_at,access_count \
                 FROM memories WHERE importance >= 4 \
                 AND (expires_at IS NULL OR expires_at > datetime('now')) \
                 ORDER BY importance DESC, updated_at DESC LIMIT 30"
            ).map_err(|e| format!("Recall critical: {}", e))?;
            let rows = stmt.query_map([], |r| Ok(row_to_memory(r)))
                .map_err(|e| format!("Recall critical: {}", e))?;
            rows.flatten().collect()
        };

        // 4. Hint-based search (if user/agent gives context about current task)
        let hint_results = if let Some(h) = hints {
            if !h.trim().is_empty() {
                self.search(h, 10, proj_ref, None, None, None).unwrap_or_default()
            } else { vec![] }
        } else { vec![] };

        // 5. Global prompt
        let global_prompt = self.get_global_prompt(proj_ref, working_dir);

        // 6. Stats summary
        let total: i64 = self.conn.query_row("SELECT COUNT(*) FROM memories", [], |r| r.get(0)).unwrap_or(0);
        let projects_count: i64 = self.conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap_or(0);

        Ok(serde_json::json!({
            "status": "recalled",
            "project": proj_ref.unwrap_or("none"),
            "stats": { "total_memories": total, "projects": projects_count, "project_memories": proj_total },
            "critical_memories": critical.iter().map(|m| serde_json::json!({
                "content": m.content, "kind": m.kind, "project": m.project,
                "tags": m.tags, "importance": m.importance
            })).collect::<Vec<_>>(),
            "project_context": proj_memories.iter().map(|m| serde_json::json!({
                "content": m.content, "kind": m.kind, "tags": m.tags, "importance": m.importance
            })).collect::<Vec<_>>(),
            "preferences": prefs.iter().map(|m| &m.content).collect::<Vec<_>>(),
            "patterns": patterns.iter().map(|m| &m.content).collect::<Vec<_>>(),
            "decisions": decisions.iter().map(|m| &m.content).collect::<Vec<_>>(),
            "hint_results": hint_results.iter().map(|r| serde_json::json!({
                "content": r.memory.content, "score": r.score, "project": r.memory.project
            })).collect::<Vec<_>>(),
            "global_prompt": global_prompt.as_deref().unwrap_or(""),
        }))
    }

    // ─── IMPORT / MIGRATE ─────────────────────────────

    pub fn import_batch(&self, memories: &[(String, String, Option<String>, Vec<String>, String)]) -> Result<usize, String> {
        let tx = self.conn.unchecked_transaction().map_err(|e| format!("Tx: {}", e))?;
        let mut count = 0;
        for (content, kind, project, tags, source) in memories {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM memories WHERE content=?1)", params![content], |r| r.get(0)
            ).unwrap_or(false);
            if exists { continue; }
            let id = Uuid::new_v4().to_string();
            let now = Utc::now().to_rfc3339();
            let tags_json = serde_json::to_string(tags).unwrap_or_else(|_| "[]".into());
            let emb = crate::embedding::embed_text(content);
            let emb_blob = crate::embedding::vec_to_blob(&emb);
            tx.execute(
                "INSERT INTO memories (id,content,kind,project,tags,source,importance,embedding,created_at,updated_at,access_count) VALUES (?1,?2,?3,?4,?5,?6,3,?7,?8,?9,0)",
                params![id, content, kind, project.as_deref(), tags_json, source, emb_blob, now, now],
            ).map_err(|e| format!("Import: {}", e))?;
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO memories_fts (rowid,content,tags,kind,project) VALUES (?1,?2,?3,?4,?5)",
                params![rowid, content, tags_json, kind, project.as_deref().unwrap_or("")],
            ).map_err(|e| format!("FTS: {}", e))?;
            if let Some(p) = project {
                let _ = tx.execute("INSERT OR IGNORE INTO projects (name,path,created_at) VALUES (?1,'',?2)", params![p, now]);
            }
            count += 1;
        }
        tx.commit().map_err(|e| format!("Commit: {}", e))?;
        Ok(count)
    }
    pub fn migrate_from_v1(&self) -> Result<usize, String> {
        let v1_dir = dirs::home_dir().ok_or("No home")?.join(DB_DIR);
        let mut batch: Vec<(String, String, Option<String>, Vec<String>, String)> = Vec::new();

        // Load global.json
        let global_path = v1_dir.join("global.json");
        if global_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&global_path) {
                if let Ok(store) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(memories) = store.get("memories").and_then(|v| v.as_array()) {
                        for m in memories { parse_v1_memory(m, None, &mut batch); }
                    }
                }
            }
        }
        // Load projects/*.json
        let projects_dir = v1_dir.join("projects");
        if projects_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&projects_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
                    let proj_name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(store) = serde_json::from_str::<serde_json::Value>(&content) {
                            if let Some(memories) = store.get("memories").and_then(|v| v.as_array()) {
                                for m in memories { parse_v1_memory(m, Some(proj_name.clone()), &mut batch); }
                            }
                        }
                    }
                }
            }
        }
        self.import_batch(&batch)
    }
} // end impl Database

// ─── Supporting types ─────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct BulkItem {
    pub content: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub project: Option<String>,
    pub tags: Option<Vec<String>>,
    #[serde(default = "default_source")]
    pub source: String,
    pub importance: Option<i32>,
    pub expires_at: Option<String>,
}
fn default_kind() -> String { "fact".into() }
fn default_source() -> String { "cursor".into() }

// ─── Row helper ───────────────────────────────────

fn row_to_memory(row: &rusqlite::Row) -> Memory {
    let tags_str: String = row.get(4).unwrap_or_default();
    let tags: Vec<String> = serde_json::from_str(&tags_str).unwrap_or_default();
    let meta_str: Option<String> = row.get(8).unwrap_or(None);
    let metadata = meta_str.and_then(|s| serde_json::from_str(&s).ok());
    Memory {
        id: row.get(0).unwrap_or_default(),
        content: row.get(1).unwrap_or_default(),
        kind: row.get(2).unwrap_or_default(),
        project: row.get(3).unwrap_or(None),
        tags,
        source: row.get(5).unwrap_or_default(),
        importance: row.get(6).unwrap_or(3),
        expires_at: row.get(7).unwrap_or(None),
        metadata,
        created_at: row.get(9).unwrap_or_default(),
        updated_at: row.get(10).unwrap_or_default(),
        last_accessed_at: row.get(11).unwrap_or(None),
        access_count: row.get(12).unwrap_or(0),
    }
}

fn parse_v1_memory(m: &serde_json::Value, project: Option<String>, batch: &mut Vec<(String, String, Option<String>, Vec<String>, String)>) {
    let c = m.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if c.is_empty() { return; }
    let k = m.get("kind").or(m.get("type")).and_then(|v| v.as_str()).unwrap_or("fact");
    let kind = match k { "context"=>"fact", "architecture"=>"decision", "component"|"workflow"=>"pattern", o=>o }.to_string();
    let tags: Vec<String> = m.get("tags").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let source = m.get("source").and_then(|v| v.as_str()).unwrap_or("v1-import").to_string();
    batch.push((c, kind, project, tags, source));
}