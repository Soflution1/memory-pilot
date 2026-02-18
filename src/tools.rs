/// MCP Tool definitions and handlers for MemoryPilot v2.1.
use serde_json::{json, Value};
use crate::db::{Database, BulkItem};
use crate::protocol::{tool_result, tool_error};

const VALID_KINDS: &[&str] = &[
    "fact", "preference", "decision", "pattern", "snippet",
    "bug", "credential", "todo", "note",
];

pub fn tool_definitions() -> Value {
    json!({ "tools": [
        {
            "name": "add_memory",
            "description": "Store a new memory with dedup. If near-duplicate exists, merges instead of creating. Kinds: fact, preference, decision, pattern, snippet, bug, credential, todo, note.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "The memory content" },
                    "kind": { "type": "string", "enum": VALID_KINDS, "default": "fact" },
                    "project": { "type": ["string","null"], "description": "Project name or null for global" },
                    "tags": { "type": "array", "items": { "type": "string" }, "default": [] },
                    "source": { "type": "string", "default": "cursor" },
                    "importance": { "type": "integer", "minimum": 1, "maximum": 5, "default": 3, "description": "1=trivial, 3=normal, 5=critical" },
                    "expires_at": { "type": ["string","null"], "description": "ISO date after which memory auto-deletes (e.g. 2025-06-01T00:00:00Z)" },
                    "metadata": { "type": ["object","null"] }
                },
                "required": ["content"]
            }
        },        {
            "name": "add_memories",
            "description": "Bulk add multiple memories in one call. Each item supports dedup. Saves context window by batching 5-20 memories.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "memories": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "kind": { "type": "string", "default": "fact" },
                            "project": { "type": ["string","null"] },
                            "tags": { "type": ["array","null"], "items": { "type": "string" } },
                            "source": { "type": "string", "default": "cursor" },
                            "importance": { "type": ["integer","null"] },
                            "expires_at": { "type": ["string","null"] }
                        },
                        "required": ["content"]
                    }}
                },
                "required": ["memories"]
            }
        },
        {
            "name": "search_memory",
            "description": "FTS5 BM25 full-text search weighted by importance. Supports prefix (svelt*) and multi-word queries. Auto-cleans expired.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "default": 10 },
                    "project": { "type": ["string","null"] },
                    "kind": { "type": ["string","null"] },
                    "tags": { "type": ["array","null"], "items": { "type": "string" } }
                },
                "required": ["query"]
            }
        },        {
            "name": "get_memory",
            "description": "Retrieve a single memory by ID.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        },
        {
            "name": "update_memory",
            "description": "Update memory content, kind, tags, importance, or expiration.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "content": { "type": ["string","null"] },
                    "kind": { "type": ["string","null"] },
                    "tags": { "type": ["array","null"], "items": { "type": "string" } },
                    "importance": { "type": ["integer","null"], "minimum": 1, "maximum": 5 },
                    "expires_at": { "type": ["string","null"] }
                },
                "required": ["id"]
            }
        },
        {
            "name": "delete_memory",
            "description": "Delete a memory by ID.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        },
        {
            "name": "list_memories",
            "description": "List memories with optional filters and pagination.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": ["string","null"] },
                    "kind": { "type": ["string","null"] },
                    "limit": { "type": "integer", "default": 20 },
                    "offset": { "type": "integer", "default": 0 }
                }
            }
        },        {
            "name": "get_project_context",
            "description": "Load full project context in ONE call: project memories + global preferences + patterns + snippets. Auto-detects project from working_dir.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": ["string","null"] },
                    "working_dir": { "type": ["string","null"], "description": "Current directory for auto-detection" }
                }
            }
        },
        {
            "name": "register_project",
            "description": "Register project with filesystem path for auto-detection.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string" }, "path": { "type": "string" }, "description": { "type": ["string","null"] } },
                "required": ["name", "path"]
            }
        },
        { "name": "list_projects", "description": "List all projects with memory counts.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "get_stats", "description": "Database statistics: totals, by kind, by project, expired count, db size.", "inputSchema": { "type": "object", "properties": {} } },
        {
            "name": "get_global_prompt",
            "description": "Load GLOBAL_PROMPT.md. Auto-scans: 1) configured path, 2) ~/.MemoryPilot/GLOBAL_PROMPT.md, 3) project root GLOBAL_PROMPT.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": ["string","null"] },
                    "working_dir": { "type": ["string","null"] }
                }
            }
        },
        {
            "name": "export_memories",
            "description": "Export memories as JSON or Markdown. Useful for backup, sharing, or injecting into Claude.ai.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": ["string","null"], "description": "Filter by project (null=all)" },
                    "format": { "type": "string", "enum": ["json", "markdown"], "default": "markdown" }
                }
            }
        },
        {
            "name": "set_config",
            "description": "Set a config value (e.g. global_prompt_path).",
            "inputSchema": { "type": "object", "properties": { "key": { "type": "string" }, "value": { "type": "string" } }, "required": ["key", "value"] }
        },
        { "name": "migrate_v1", "description": "Import from v1 JSON files. Skips duplicates.", "inputSchema": { "type": "object", "properties": {} } },
        { "name": "cleanup_expired", "description": "Manually remove all expired memories.", "inputSchema": { "type": "object", "properties": {} } }
    ]})
}
/// Handle a tools/call request.
pub fn handle_tool_call(db: &Database, name: &str, args: &Value) -> Value {
    match name {
        "add_memory" => handle_add(db, args),
        "add_memories" => handle_add_bulk(db, args),
        "search_memory" => handle_search(db, args),
        "get_memory" => handle_get(db, args),
        "update_memory" => handle_update(db, args),
        "delete_memory" => handle_delete(db, args),
        "list_memories" => handle_list(db, args),
        "get_project_context" => handle_project_context(db, args),
        "register_project" => handle_register_project(db, args),
        "list_projects" => handle_list_projects(db),
        "get_stats" => handle_stats(db),
        "get_global_prompt" => handle_global_prompt(db, args),
        "export_memories" => handle_export(db, args),
        "set_config" => handle_set_config(db, args),
        "migrate_v1" => handle_migrate(db),
        "cleanup_expired" => handle_cleanup(db),
        _ => tool_error(&format!("Unknown tool: {}", name)),
    }
}

fn handle_add(db: &Database, args: &Value) -> Value {
    let content = match args.get("content").and_then(|v| v.as_str()) {
        Some(c) if !c.trim().is_empty() => c,
        _ => return tool_error("content is required"),
    };
    let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("fact");
    if !VALID_KINDS.contains(&kind) { return tool_error(&format!("Invalid kind '{}'. Valid: {:?}", kind, VALID_KINDS)); }
    let project = args.get("project").and_then(|v| v.as_str());
    let tags: Vec<String> = args.get("tags").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("cursor");
    let importance = args.get("importance").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
    let expires_at = args.get("expires_at").and_then(|v| v.as_str());
    let metadata = args.get("metadata").filter(|v| !v.is_null());

    match db.add_memory(content, kind, project, &tags, source, importance, expires_at, metadata) {
        Ok((mem, was_merged)) => {
            let mut result = serde_json::to_value(&mem).unwrap_or(json!({}));
            if was_merged { result.as_object_mut().map(|o| o.insert("_merged".into(), json!(true))); }
            tool_result(&serde_json::to_string_pretty(&result).unwrap())
        }
        Err(e) => tool_error(&e),
    }
}
fn handle_add_bulk(db: &Database, args: &Value) -> Value {
    let items: Vec<BulkItem> = match args.get("memories").and_then(|v| serde_json::from_value::<Vec<BulkItem>>(v.clone()).ok()) {
        Some(items) if !items.is_empty() => items,
        _ => return tool_error("memories array is required and cannot be empty"),
    };
    match db.add_memories_bulk(&items) {
        Ok((added, merged, skipped)) => {
            tool_result(&format!("Bulk complete: {} added, {} merged (dedup), {} skipped. Total processed: {}.",
                added.len(), merged, skipped, items.len()))
        }
        Err(e) => tool_error(&e),
    }
}

fn handle_search(db: &Database, args: &Value) -> Value {
    let query = match args.get("query").and_then(|v| v.as_str()) {
        Some(q) if !q.trim().is_empty() => q,
        _ => return tool_error("query is required"),
    };
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let project = args.get("project").and_then(|v| v.as_str());
    let kind = args.get("kind").and_then(|v| v.as_str());
    let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    match db.search(query, limit, project, kind, tags.as_deref()) {
        Ok(results) => {
            let output = json!({ "query": query, "count": results.len(),
                "results": results.iter().map(|r| json!({
                    "id": r.memory.id, "content": r.memory.content, "kind": r.memory.kind,
                    "project": r.memory.project, "tags": r.memory.tags, "score": r.score, "importance": r.memory.importance,
                })).collect::<Vec<_>>()
            });
            tool_result(&serde_json::to_string_pretty(&output).unwrap())
        }
        Err(e) => tool_error(&e),
    }
}

fn handle_get(db: &Database, args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_str()) { Some(i) => i, _ => return tool_error("id required") };
    match db.get_memory(id) {
        Ok(Some(mem)) => tool_result(&serde_json::to_string_pretty(&mem).unwrap()),
        Ok(None) => tool_error(&format!("Not found: {}", id)),
        Err(e) => tool_error(&e),
    }
}
fn handle_update(db: &Database, args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_str()) { Some(i) => i, _ => return tool_error("id required") };
    let content = args.get("content").and_then(|v| v.as_str());
    let kind = args.get("kind").and_then(|v| v.as_str());
    let tags: Option<Vec<String>> = args.get("tags").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
    let importance = args.get("importance").and_then(|v| v.as_i64()).map(|i| i as i32);
    let expires_at = args.get("expires_at").and_then(|v| v.as_str());
    match db.update_memory_full(id, content, kind, tags.as_deref(), importance, expires_at) {
        Ok(Some(mem)) => tool_result(&serde_json::to_string_pretty(&mem).unwrap()),
        Ok(None) => tool_error(&format!("Not found: {}", id)),
        Err(e) => tool_error(&e),
    }
}

fn handle_delete(db: &Database, args: &Value) -> Value {
    let id = match args.get("id").and_then(|v| v.as_str()) { Some(i) => i, _ => return tool_error("id required") };
    match db.delete_memory(id) {
        Ok(true) => tool_result(&format!("Deleted: {}", id)),
        Ok(false) => tool_error(&format!("Not found: {}", id)),
        Err(e) => tool_error(&e),
    }
}

fn handle_list(db: &Database, args: &Value) -> Value {
    let project = args.get("project").and_then(|v| v.as_str());
    let kind = args.get("kind").and_then(|v| v.as_str());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    match db.list_memories(project, kind, limit, offset) {
        Ok((memories, total)) => {
            tool_result(&serde_json::to_string_pretty(&json!({"total":total,"count":memories.len(),"offset":offset,"memories":memories})).unwrap())
        }
        Err(e) => tool_error(&e),
    }
}
fn handle_project_context(db: &Database, args: &Value) -> Value {
    let project = args.get("project").and_then(|v| v.as_str());
    let working_dir = args.get("working_dir").and_then(|v| v.as_str());
    match db.get_project_context(project, working_dir) {
        Ok(ctx) => tool_result(&serde_json::to_string_pretty(&ctx).unwrap()),
        Err(e) => tool_error(&e),
    }
}

fn handle_register_project(db: &Database, args: &Value) -> Value {
    let name = match args.get("name").and_then(|v| v.as_str()) { Some(n) => n, _ => return tool_error("name required") };
    let path = match args.get("path").and_then(|v| v.as_str()) { Some(p) => p, _ => return tool_error("path required") };
    let desc = args.get("description").and_then(|v| v.as_str());
    match db.register_project(name, path, desc) {
        Ok(proj) => tool_result(&serde_json::to_string_pretty(&proj).unwrap()),
        Err(e) => tool_error(&e),
    }
}

fn handle_list_projects(db: &Database) -> Value {
    match db.list_projects() {
        Ok(p) => tool_result(&serde_json::to_string_pretty(&p).unwrap()),
        Err(e) => tool_error(&e),
    }
}

fn handle_stats(db: &Database) -> Value {
    match db.stats() {
        Ok(s) => tool_result(&serde_json::to_string_pretty(&s).unwrap()),
        Err(e) => tool_error(&e),
    }
}

fn handle_global_prompt(db: &Database, args: &Value) -> Value {
    let project = args.get("project").and_then(|v| v.as_str());
    let working_dir = args.get("working_dir").and_then(|v| v.as_str());
    match db.get_global_prompt(project, working_dir) {
        Some(prompt) => tool_result(&prompt),
        None => tool_error("No GLOBAL_PROMPT.md found. Place it in ~/.MemoryPilot/ or project root, or use set_config(key='global_prompt_path')."),
    }
}

fn handle_export(db: &Database, args: &Value) -> Value {
    let project = args.get("project").and_then(|v| v.as_str());
    let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("markdown");
    match db.export_memories(project, format) {
        Ok(output) => tool_result(&output),
        Err(e) => tool_error(&e),
    }
}

fn handle_set_config(db: &Database, args: &Value) -> Value {
    let key = match args.get("key").and_then(|v| v.as_str()) { Some(k) => k, _ => return tool_error("key required") };
    let value = match args.get("value").and_then(|v| v.as_str()) { Some(v) => v, _ => return tool_error("value required") };
    match db.set_config(key, value) {
        Ok(()) => tool_result(&format!("Config '{}' = '{}'", key, value)),
        Err(e) => tool_error(&e),
    }
}

fn handle_migrate(db: &Database) -> Value {
    match db.migrate_from_v1() {
        Ok(count) => tool_result(&format!("Migrated {} memories from v1 to SQLite.", count)),
        Err(e) => tool_error(&format!("Migration failed: {}", e)),
    }
}

fn handle_cleanup(db: &Database) -> Value {
    match db.cleanup_expired() {
        Ok(count) => tool_result(&format!("Cleaned up {} expired memories.", count)),
        Err(e) => tool_error(&e),
    }
}