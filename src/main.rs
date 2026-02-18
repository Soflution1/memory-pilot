/// MemoryPilot v2.1 — High-performance MCP memory server.
/// SQLite FTS5, project-scoped, dedup, importance, TTL, bulk ops, export.
/// (c) SOFLUTION LTD — MIT License
mod db;
mod protocol;
mod tools;

use std::io::{self, BufRead, Write};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use serde_json::json;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SERVER_NAME: &str = "MemoryPilot";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-v") { println!("MemoryPilot v{}", VERSION); return; }
    if args.iter().any(|a| a == "--help" || a == "-h") { print_help(); return; }
    if args.iter().any(|a| a == "--migrate") { run_migrate(); return; }
    run_mcp_server();
}

fn run_mcp_server() {
    let db = match db::Database::open() {
        Ok(d) => d, Err(e) => { eprintln!("DB error: {}", e); std::process::exit(1); }
    };
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();    for line in stdin.lock().lines() {
        let line = match line { Ok(l) if !l.trim().is_empty() => l, Ok(_) => continue, Err(_) => break };
        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse: {}", e));
                let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap());
                let _ = out.flush(); continue;
            }
        };
        let response = handle_request(&db, &request);
        let _ = writeln!(out, "{}", serde_json::to_string(&response).unwrap());
        let _ = out.flush();
    }
}

fn handle_request(db: &db::Database, req: &JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => JsonRpcResponse::success(req.id.clone(), json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": VERSION }
        })),
        "notifications/initialized" => JsonRpcResponse::success(req.id.clone(), json!({})),
        "tools/list" => JsonRpcResponse::success(req.id.clone(), tools::tool_definitions()),
        "tools/call" => {
            let name = req.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = req.params.get("arguments").cloned().unwrap_or(json!({}));
            JsonRpcResponse::success(req.id.clone(), tools::handle_tool_call(db, name, &args))
        }
        "ping" => JsonRpcResponse::success(req.id.clone(), json!({})),
        _ => JsonRpcResponse::error(req.id.clone(), -32601, format!("Unknown: {}", req.method)),
    }
}
fn run_migrate() {
    let db = match db::Database::open() { Ok(d) => d, Err(e) => { eprintln!("DB error: {}", e); std::process::exit(1); } };
    match db.migrate_from_v1() {
        Ok(n) => println!("✓ Migrated {} memories from v1 JSON to SQLite.", n),
        Err(e) => { eprintln!("✗ Failed: {}", e); std::process::exit(1); }
    }
}

fn print_help() {
    println!("MemoryPilot v{} — MCP memory server with SQLite FTS5", VERSION);
    println!();
    println!("USAGE:");
    println!("  MemoryPilot              Start MCP stdio server");
    println!("  MemoryPilot --migrate    Migrate v1 JSON data to SQLite");
    println!("  MemoryPilot --version    Show version");
    println!("  MemoryPilot --help       Show this help");
    println!();
    println!("MCP TOOLS (16):");
    println!("  add_memory          Store with auto-dedup, importance (1-5), TTL");
    println!("  add_memories        Bulk add multiple memories in 1 call");
    println!("  search_memory       FTS5 BM25 × importance ranking");
    println!("  get_memory          Retrieve by ID");
    println!("  update_memory       Update content/kind/tags/importance/TTL");
    println!("  delete_memory       Delete by ID");
    println!("  list_memories       List with filters & pagination");
    println!("  get_project_context Full context in 1 call + auto-detect");
    println!("  register_project    Register project path");
    println!("  list_projects       List projects with counts");
    println!("  get_stats           Database statistics");
    println!("  get_global_prompt   Auto-discover GLOBAL_PROMPT.md");
    println!("  export_memories     Export as JSON or Markdown");
    println!("  set_config          Set config values");
    println!("  cleanup_expired     Remove expired memories");
    println!("  migrate_v1          Import from v1 JSON files");
    println!();
    println!("STORAGE:  ~/.MemoryPilot/memory.db");
    println!("SEARCH:   SQLite FTS5 with BM25 × importance");
    println!("BUILT BY: SOFLUTION LTD");
}