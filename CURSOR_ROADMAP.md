# MemoryPilot v3.0 — God-Tier Integration Prompt for Cursor

## Situation

MemoryPilot is a Rust MCP memory server. The core is working (v2.1): SQLite + FTS5, 16 tools, dedup, recall, project context. Three new modules have been coded but are **NOT integrated**:

- `src/embedding.rs` — TF-IDF vector engine (384-dim, cosine similarity, RRF fusion). Written, tested, not wired.
- `src/graph.rs` — Entity extraction + relationship inference. Written, not wired.
- `src/gc.rs` — Garbage collection scoring + memory merging. Written, not wired.

The `notify` crate is in Cargo.toml but no file watcher code exists yet.

**Your job: wire everything together.** The modules are good. What's missing is the glue: schema upgrades, DB methods, tool handlers, and the watcher.

## File Locations

- Source: `/Users/antoinepinelli/Cursor/Apps/MemoryPilot/src/`
- Database: `~/.MemoryPilot/memory.db`
- Binary: built to `target/release/MemoryPilot`

## Source Architecture

```
src/main.rs        — CLI + MCP stdio server loop
src/db.rs          — SQLite database: CRUD, search (BM25), dedup, recall, projects, export
src/tools.rs       — MCP tool definitions (16 tools) + handlers
src/protocol.rs    — JSON-RPC types
src/embedding.rs   — TF-IDF embeddings (NOT WIRED)
src/graph.rs       — Entity extraction + relation inference (NOT WIRED)
src/gc.rs          — GC scoring + memory merge (NOT WIRED)
```

---

## PILLAR 1 — Hybrid Search (BM25 + TF-IDF RRF)

### What exists
`src/embedding.rs`: `embed_text()`, `cosine_similarity()`, `rrf_score()`, `vec_to_blob()`, `blob_to_vec()`. Working with tests.

### What to do

**1a. Schema upgrade** in `src/db.rs` → `upgrade_schema()`:
Add columns if missing (use existing pattern: try SELECT first):
```sql
ALTER TABLE memories ADD COLUMN embedding BLOB;
ALTER TABLE memories ADD COLUMN last_accessed_at TEXT;
ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;
```

**1b. Generate embeddings on insert** in `db.rs` → `add_memory()`:
After the INSERT, compute and store embedding:
```rust
use crate::embedding::{embed_text, vec_to_blob};
let emb = embed_text(content);
let blob = vec_to_blob(&emb);
self.conn.execute("UPDATE memories SET embedding = ?1 WHERE id = ?2", params![blob, id])?;
```

**1c. Hybrid search** in `db.rs` → `search()`:
Current search is BM25-only. Make it hybrid:
1. Run existing FTS5 BM25 query → ranked results
2. Compute query embedding: `embed_text(query)`
3. For each BM25 result with an embedding BLOB, compute cosine similarity
4. Also scan top-200 memories by cosine similarity alone (catches semantic matches BM25 misses)
5. Merge both lists using RRF: `rrf_score(bm25_rank, vector_rank)`
6. Sort by final RRF score descending, return top `limit`
This should be the DEFAULT behavior. No separate tool needed.

**1d. Track access** in `search()` and `get_memory()`:
```sql
UPDATE memories SET last_accessed_at = ?1, access_count = access_count + 1 WHERE id = ?2
```

**1e. Backfill existing memories:**
Add `backfill_embeddings()` method: find all `embedding IS NULL`, compute and store.
Call it once in `Database::open()` after `upgrade_schema()`.
Also expose as `MemoryPilot --backfill` CLI command.

---

## PILLAR 2 — Knowledge Graph

### What exists
`src/graph.rs`: `extract_entities()`, `infer_relation()`. Detects projects, techs, components, file paths.

### What to do

**2a. Schema** in `db.rs` → `init_schema()`:
Add to the existing CREATE TABLE batch:
```sql
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
```

**2b. Auto-extract entities on insert** in `db.rs` → `add_memory()`:
After inserting the memory:
```rust
use crate::graph::{extract_entities, infer_relation};
let entities = extract_entities(content, project);
for entity in &entities {
    self.conn.execute(
        "INSERT OR IGNORE INTO memory_entities (memory_id, entity_kind, entity_value) VALUES (?1, ?2, ?3)",
        params![id, entity.kind, entity.value],
    )?;
}
```

**2c. Auto-link related memories:**
After extracting entities, find other memories sharing the same entities:
```rust
for entity in &entities {
    let related_ids: Vec<String> = /* SELECT DISTINCT memory_id FROM memory_entities
        WHERE entity_value = ?1 AND memory_id != ?2 LIMIT 10 */;
    for related_id in &related_ids {
        let related_kind = /* get kind of related_id */;
        let relation = infer_relation(kind, &related_kind);
        /* INSERT OR IGNORE INTO memory_links ... */
    }
}
```

**2d. Graph traversal** method in `db.rs`:
```rust
pub fn get_related_memories(&self, memory_id: &str, depth: usize) -> Result<Vec<(Memory, String)>, String>
```
Returns (memory, relation_type) for all memories linked directly (depth=1) or transitively (depth=2).

**2e. Include in search results:**
Add `related_count` field to SearchResult: count of links per memory.

---

## PILLAR 3 — Garbage Collection

### What exists
`src/gc.rs`: `gc_score()`, `merge_memories()`, `GcConfig`, `GcReport`, stopword list.

### What to do

**3a. GC runner** in `db.rs`:
```rust
pub fn run_gc(&self, config: &gc::GcConfig) -> Result<gc::GcReport, String> {
    // 1. Remove expired memories (existing cleanup_expired)
    // 2. Find candidates: old + low importance + compressible kinds
    //    Use gc::gc_score() to rank, filter score > 0.6
    // 3. Group candidates by (project, kind)
    // 4. For groups with 3+ memories: gc::merge_memories() → delete originals, insert merged
    // 5. Clean orphaned entities: DELETE FROM memory_entities WHERE memory_id NOT IN (SELECT id FROM memories)
    // 6. Clean orphaned links: same pattern
    // 7. VACUUM if significant deletions
    // Return GcReport with counts
}
```

**3b. New MCP tool `run_gc`** in `tools.rs`:
```json
{
    "name": "run_gc",
    "description": "Garbage collection: merge old low-importance memories, remove expired, clean orphans. Keeps DB dense.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "age_days": { "type": "integer", "default": 30 },
            "importance_threshold": { "type": "integer", "default": 3 },
            "dry_run": { "type": "boolean", "default": false }
        }
    }
}
```

**3c. Auto-GC** (low priority):
On startup, check if last GC > 7 days ago (store `last_gc` in config table). If so, run GC with default config.

---

## PILLAR 4 — `get_project_brain` Tool

The crown jewel. One tool call → condensed JSON brain dump under 1500 tokens.

**4a. New method** in `db.rs`:
```rust
pub fn get_project_brain(&self, project: &str) -> Result<serde_json::Value, String> {
    // 1. Tech stack: SELECT DISTINCT entity_value FROM memory_entities
    //    WHERE entity_kind='tech' AND memory_id IN (SELECT id FROM memories WHERE project=?1)
    // 2. Architecture: SELECT content FROM memories WHERE project=?1
    //    AND kind IN ('architecture','decision') AND importance >= 3
    //    ORDER BY importance DESC, updated_at DESC LIMIT 10
    // 3. Active bugs: SELECT content FROM memories WHERE project=?1 AND kind='bug'
    //    AND (expires_at IS NULL OR expires_at > datetime('now'))
    //    ORDER BY importance DESC LIMIT 5
    // 4. Recent changes: SELECT content FROM memories WHERE project=?1
    //    AND updated_at > datetime('now','-7 days')
    //    ORDER BY updated_at DESC LIMIT 10
    // 5. Preferences/patterns: SELECT content FROM memories
    //    WHERE (project=?1 OR project IS NULL) AND kind IN ('preference','pattern')
    //    AND importance >= 3 ORDER BY importance DESC LIMIT 10
    // 6. Key components: SELECT DISTINCT entity_value FROM memory_entities
    //    WHERE entity_kind IN ('component','file') AND memory_id IN (...)
    //    LIMIT 15

    Ok(json!({
        "project": project,
        "tech_stack": techs,
        "core_architecture": architecture,
        "active_bugs": bugs,
        "recent_changes": recent,
        "preferences_and_patterns": preferences,
        "key_components": components
    }))
}
```

**4b. MCP tool** in `tools.rs`:
```json
{
    "name": "get_project_brain",
    "description": "INSTANT PROJECT BRAIN — Dense JSON summary (<1500 tokens): tech stack, architecture, active bugs, recent changes, preferences, key components. Use at start of focused work.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "project": { "type": ["string","null"], "description": "Project name (or null for auto-detect)" },
            "working_dir": { "type": ["string","null"], "description": "Auto-detect project from path" }
        }
    }
}
```
In the handler: auto-detect project from working_dir using existing `db.detect_project()` if project is null.

---

## PILLAR 5 — File Watcher

### What exists
`notify` v6 crate is in Cargo.toml. No watcher code exists.

### What to do

**5a. Create `src/watcher.rs`:**
A background thread that watches a directory for file changes and maintains a ring buffer of recent changes.

```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;

pub struct FileWatcherState {
    pub recent_changes: VecDeque<FileChange>,
}

pub struct FileChange {
    pub path: String,
    pub filename: String,
    pub timestamp: String,
}

impl FileWatcherState {
    pub fn new() -> Self { Self { recent_changes: VecDeque::with_capacity(20) } }

    pub fn push(&mut self, change: FileChange) {
        if self.recent_changes.len() >= 20 { self.recent_changes.pop_front(); }
        self.recent_changes.push_back(change);
    }

    /// Keywords from recent file changes for search boosting.
    pub fn get_boost_keywords(&self) -> Vec<String> {
        self.recent_changes.iter()
            .flat_map(|c| {
                let stem = c.filename.split('.').next().unwrap_or(&c.filename);
                split_camel_kebab(stem)
            })
            .collect()
    }
}

pub fn start_watcher(dir: &str) -> Option<Arc<Mutex<FileWatcherState>>> {
    let state = Arc::new(Mutex::new(FileWatcherState::new()));
    let state_clone = state.clone();
    let dir_path = std::path::PathBuf::from(dir);

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res { let _ = tx.send(event); }
        }).ok()?;
        watcher.watch(&dir_path, RecursiveMode::Recursive).ok()?;

        for event in rx {
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) { continue; }
            for path in &event.paths {
                let path_str = path.to_string_lossy();
                // Skip .git, node_modules, target, hidden files
                if path_str.contains("/.") || path_str.contains("/node_modules/")
                    || path_str.contains("/target/") { continue; }
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if filename.is_empty() { continue; }
                if let Ok(mut s) = state_clone.lock() {
                    s.push(FileChange {
                        path: path_str.to_string(),
                        filename,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                }
            }
        }
        Some(()) // type hint for closure
    });

    Some(state)
}
```

**5b. Integration into search:**
Pass the watcher state to `search()`. If boost keywords match any FTS result, multiply its score:
```rust
// In search(), after computing RRF scores:
if let Some(watcher) = &watcher_state {
    let boost_words = watcher.lock().unwrap().get_boost_keywords();
    for result in &mut results {
        let content_lower = result.memory.content.to_lowercase();
        let boost = boost_words.iter().filter(|w| content_lower.contains(w.as_str())).count();
        if boost > 0 {
            result.score *= 1.0 + (boost as f64 * 0.2); // 20% boost per matching keyword
        }
    }
}
```

**5c. New tool `get_file_context`** in `tools.rs`:
```json
{
    "name": "get_file_context",
    "description": "Get memories related to recently modified files in the working directory. Uses the file watcher to know what you're working on.",
    "inputSchema": {
        "type": "object",
        "properties": {
            "working_dir": { "type": "string" }
        },
        "required": ["working_dir"]
    }
}
```
Handler: Start watcher if not running for that dir, get boost keywords, search memories with those keywords.

**5d. Wire into main.rs:**
The watcher needs to be started when a `recall` or `get_project_brain` call provides a `working_dir`. Store the state in a static or pass through the call chain.
Option: Use `Arc<Mutex<Option<Arc<Mutex<FileWatcherState>>>>>` in main and pass to handlers. Or simpler: start watcher on first `recall` call that has a working_dir.

---

## Integration Order

Do these in order. Each is a separate commit.

1. **Schema upgrade** — Add columns + tables (embedding, access tracking, links, entities). This is foundational.
2. **Embeddings on insert** — Wire `embed_text()` into `add_memory()`. Add `--backfill` command.
3. **Hybrid search** — Replace BM25-only search with RRF fusion. Test with real queries.
4. **Entity extraction + linking** — Wire `extract_entities()` into `add_memory()`. Create links table population.
5. **Graph traversal** — Add `get_related_memories()` method. Include related_count in search results.
6. **get_project_brain** — New tool combining entities, architecture, bugs, recent changes.
7. **GC runner** — Wire `gc.rs` into a `run_gc()` method + tool. Test with dry_run.
8. **File watcher** — Create `watcher.rs`, wire into search boosting.
9. **Auto-GC on startup** — Low priority polish.

## Build & Test

```bash
cd /Users/antoinepinelli/Cursor/Apps/MemoryPilot

# Build
cargo build --release

# Test
cargo test

# Run manually (stdio mode, for testing with pipe)
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' | ./target/release/MemoryPilot

# Test search
echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_memory","arguments":{"query":"auth login"}}}' | ./target/release/MemoryPilot

# Deploy
cp target/release/MemoryPilot ~/.local/bin/MemoryPilot
chmod +x ~/.local/bin/MemoryPilot
xattr -cr ~/.local/bin/MemoryPilot
```

## Rules

- Keep the binary small. No heavy crates (no ONNX, no Bert). The TF-IDF approach in embedding.rs is intentional.
- Don't break existing tools. All 16 current tools must keep working.
- The hybrid search should be transparent: same `search_memory` tool, just smarter results.
- Use rusqlite transactions for batch operations (GC merge, backfill).
- All new tables need ON DELETE CASCADE so `delete_memory` cleans up links/entities automatically.
- Version in Cargo.toml stays at 3.0.0 until all 5 pillars are wired.
- Then bump to 3.1.0 and commit.
