<p align="center">
  <img src="static/banner.svg" alt="MemoryPilot" width="900"/>
</p>

<p align="center">
  <strong>High-performance MCP memory server for AI agents.</strong><br>
  <sub>Persistent searchable memory · Project-aware · Single binary · Zero dependencies</sub>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/language-Rust-orange" alt="Rust"/>
  <img src="https://img.shields.io/badge/search-SQLite_FTS5-blueviolet" alt="SQLite FTS5"/>
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT"/>
  <img src="https://img.shields.io/badge/binary-~2.2MB-yellow" alt="Binary size"/>
</p>

---

## Why

AI coding assistants forget everything between sessions. MemoryPilot gives them persistent, searchable memory with project awareness.

**vs MCP Memory (official):** 1 tool call to store (vs 3), BM25 ranked search (vs unranked), project scoping, auto-dedup, importance weighting, TTL expiration. Single 2.2MB binary vs Node.js + npm.

## Features

- **SQLite FTS5** full-text search with BM25 ranking × importance weighting
- **Project-scoped** memories with auto-detection from working directory
- **Auto-deduplication** (85% similarity threshold, no duplicates)
- **16 MCP tools** — add, bulk add, search, update, delete, list, export, and more
- **9 memory types** — fact, preference, decision, pattern, snippet, bug, credential, todo, note
- **TTL/expiration** — memories auto-cleanup when expired
- **Export** — JSON or Markdown with importance stars and type grouping
- **Global prompt** — auto-discovers `GLOBAL_PROMPT.md` from `~/.memory-pilot/`
- **Migration** — import from v1 JSON format

## Performance

| Metric | MemoryPilot | MCP Memory (Node.js) |
|--------|------------|---------------------|
| Startup | 1-2ms | 50-100ms |
| RAM | 5MB | 30MB |
| Binary | 2.2MB | 200MB+ (node_modules) |
| Search | FTS5 BM25 (C speed) | Unranked filter |
| Storage | SQLite ACID | JSON in npm cache |

## Install

```bash
cargo build --release
cp target/release/memory-pilot ~/.local/bin/
```

## Cursor Integration

Add to `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "memory-pilot": {
      "command": "/path/to/memory-pilot",
      "args": []
    }
  }
}
```

## MCP Tools (16)

| Tool | Description |
|------|-------------|
| `add_memory` | Store with auto-dedup, importance (1-5), TTL |
| `add_memories` | Bulk add multiple memories in 1 call |
| `search_memory` | FTS5 BM25 × importance ranked search |
| `get_memory` | Retrieve by ID |
| `update_memory` | Update content/kind/tags/importance/TTL |
| `delete_memory` | Delete by ID |
| `list_memories` | List with filters & pagination |
| `get_project_context` | Full context in 1 call + auto-detect project |
| `register_project` | Register project filesystem path |
| `list_projects` | List projects with memory counts |
| `get_stats` | Database statistics |
| `get_global_prompt` | Auto-discover GLOBAL_PROMPT.md |
| `export_memories` | Export as JSON or Markdown |
| `set_config` | Set config values |
| `cleanup_expired` | Remove expired memories |
| `migrate_v1` | Import from v1 JSON files |

## CLI

```bash
memory-pilot              # Start MCP stdio server
memory-pilot --migrate    # Migrate v1 JSON data to SQLite
memory-pilot --version    # Show version
memory-pilot --help       # Show help
```

## Storage

- Database: `~/.memory-pilot/memory.db`
- Global prompt: `~/.memory-pilot/GLOBAL_PROMPT.md`

## License

MIT — Built by [SOFLUTION LTD](https://soflution.com)
