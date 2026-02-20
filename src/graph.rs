/// MemoryPilot v3.0 — Knowledge Graph Engine.
/// Automatic entity extraction, relationship management, and graph traversal.
use std::collections::HashSet;

/// Extracted entity from memory content.
#[derive(Debug, Clone)]
pub struct Entity {
    pub kind: &'static str, // "project", "tech", "component", "file", "person"
    pub value: String,
}

/// Known tech patterns for auto-detection.
const TECH_PATTERNS: &[&str] = &[
    "svelte", "sveltekit", "svelte 5", "react", "vue", "next", "nuxt", "astro",
    "supabase", "firebase", "postgresql", "sqlite", "redis", "mongodb",
    "tailwind", "css", "sass", "bootstrap",
    "rust", "typescript", "javascript", "python", "swift", "go", "java",
    "cloudflare", "vercel", "netlify", "aws", "hetzner", "docker",
    "stripe", "auth", "jwt", "oauth", "better-auth",
    "onnx", "bert", "openai", "claude", "llm", "mcp",
    "tauri", "electron", "flutter", "xcode",
    "git", "github", "npm", "cargo", "pnpm",
];

/// Known component patterns (file-like).
const COMPONENT_HINTS: &[&str] = &[
    "component", "page", "layout", "modal", "button", "form", "input",
    "header", "footer", "sidebar", "nav", "card", "table", "dialog",
    "dashboard", "settings", "profile", "auth", "login", "signup",
];

/// Extract entities from memory content automatically.
/// Detects: projects, technologies, components, file paths, people.
pub fn extract_entities(content: &str, project: Option<&str>) -> Vec<Entity> {
    let lower = content.to_lowercase();
    let mut entities: Vec<Entity> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // 1. Project (from parameter or content)
    if let Some(p) = project {
        if seen.insert(format!("project:{}", p.to_lowercase())) {
            entities.push(Entity { kind: "project", value: p.to_string() });
        }
    }

    // 2. Technologies
    for tech in TECH_PATTERNS {
        if lower.contains(tech) && seen.insert(format!("tech:{}", tech)) {
            entities.push(Entity { kind: "tech", value: tech.to_string() });
        }
    }

    // 3. File paths (detect patterns like src/foo/bar.ts, lib/components/X.svelte)
    for word in content.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-');
        if w.contains('/') && w.contains('.') && w.len() > 4 {
            if seen.insert(format!("file:{}", w.to_lowercase())) {
                entities.push(Entity { kind: "file", value: w.to_string() });
            }
        }
        // Also detect .svelte, .ts, .rs files without path
        if (w.ends_with(".svelte") || w.ends_with(".ts") || w.ends_with(".tsx")
            || w.ends_with(".rs") || w.ends_with(".py") || w.ends_with(".js"))
            && w.len() > 4 && !w.starts_with('.')
        {
            if seen.insert(format!("file:{}", w.to_lowercase())) {
                entities.push(Entity { kind: "file", value: w.to_string() });
            }
        }
    }

    // 4. Components (UI component names)
    for hint in COMPONENT_HINTS {
        if lower.contains(hint) {
            // Try to find the actual component name (PascalCase or kebab-case near the hint)
            for word in content.split_whitespace() {
                let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
                if w.len() > 2 && (w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                    || w.contains('-') || w.contains('_'))
                    && lower_contains_near(&lower, hint, &w.to_lowercase(), 50)
                {
                    if seen.insert(format!("component:{}", w.to_lowercase())) {
                        entities.push(Entity { kind: "component", value: w.to_string() });
                    }
                }
            }
        }
    }

    entities
}

/// Infer relationship type between two memories based on their kinds.
pub fn infer_relation(source_kind: &str, target_kind: &str) -> &'static str {
    match (source_kind, target_kind) {
        ("bug", "decision") | ("bug", "architecture") => "resolved_by",
        ("decision", "bug") => "resolves",
        ("bug", "snippet") => "fixed_by",
        ("snippet", "bug") => "fixes",
        ("decision", "architecture") | ("decision", "pattern") => "implements",
        ("architecture", "decision") => "decided_by",
        ("todo", _) => "depends_on",
        (_, "todo") => "blocks",
        _ => "relates_to",
    }
}

/// Check if two substrings appear within `distance` chars of each other.
fn lower_contains_near(text: &str, a: &str, b: &str, distance: usize) -> bool {
    if let Some(pos_a) = text.find(a) {
        if let Some(pos_b) = text.find(b) {
            let diff = if pos_a > pos_b { pos_a - pos_b } else { pos_b - pos_a };
            return diff <= distance;
        }
    }
    false
}
