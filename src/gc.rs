/// MemoryPilot v3.0 — Garbage Collection & Memory Compression.
/// Heuristic-based cleanup: merges old low-importance memories, keeps base dense.
/// Runs as background thread or on-demand via tool.

/// Result of a GC cycle.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GcReport {
    pub expired_removed: usize,
    pub groups_merged: usize,
    pub memories_compressed: usize,
    pub orphan_links_removed: usize,
    pub db_size_before: u64,
    pub db_size_after: u64,
}

/// Configuration for GC behavior.
pub struct GcConfig {
    /// Memories older than this (days) with importance < threshold are candidates.
    pub age_days: i64,
    /// Importance threshold: memories below this are candidates for merge.
    pub importance_threshold: i32,
    /// Maximum memories in a merge group.
    pub max_merge_group: usize,
    /// Kinds eligible for compression.
    pub compressible_kinds: Vec<String>,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            age_days: 30,
            importance_threshold: 3,
            max_merge_group: 10,
            compressible_kinds: vec![
                "bug".into(), "snippet".into(), "note".into(), "todo".into(),
            ],
        }
    }
}

/// Merge a group of related old memories into a single condensed memory.
/// Pure heuristic summarization — no LLM needed.
pub fn merge_memories(contents: &[String], kind: &str, project: Option<&str>) -> String {
    if contents.len() == 1 {
        return contents[0].clone();
    }

    // Count word frequency across all memories (document frequency, not raw)
    let mut word_freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for c in contents {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for w in c.split_whitespace() {
            let w = w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
            if w.len() > 3 && !is_stopword(&w) && seen.insert(w.clone()) {
                *word_freq.entry(w).or_default() += 1;
            }
        }
    }

    // Top 5 keywords = subject
    let mut top_words: Vec<(String, usize)> = word_freq.into_iter().collect();
    top_words.sort_by(|a, b| b.1.cmp(&a.1));
    let subject: String = top_words.iter()
        .take(5)
        .map(|(w, _)| w.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    // Build condensed summary
    let project_prefix = project.map(|p| format!("[{}] ", p)).unwrap_or_default();
    let kind_label = match kind {
        "bug" => "Bugs",
        "snippet" => "Code snippets",
        "note" => "Notes",
        "todo" => "TODOs",
        _ => "Items",
    };

    // Take first sentence of each memory as bullet point
    let bullets: Vec<String> = contents.iter()
        .filter_map(|c| {
            let trimmed = c.trim();
            // Take first sentence or first 120 chars
            let end = trimmed.find(". ")
                .map(|i| i + 1)
                .unwrap_or_else(|| trimmed.len().min(120));
            let sentence = &trimmed[..end];
            if sentence.len() > 5 { Some(format!("- {}", sentence)) } else { None }
        })
        .take(8) // Max 8 bullets
        .collect();

    format!(
        "{}[MERGED] {} related to: {}. ({} items compressed)\n{}",
        project_prefix, kind_label, subject,
        contents.len(), bullets.join("\n")
    )
}

/// Score a memory for GC candidacy (higher = more likely to be collected).
/// Returns 0.0-1.0.
pub fn gc_score(importance: i32, age_days: i64, kind: &str, _config: &GcConfig) -> f64 {
    // Base score from importance (lower importance = higher GC score)
    let importance_score = 1.0 - ((importance as f64 - 1.0) / 4.0); // 1->1.0, 5->0.0

    // Age factor (older = higher score)
    let age_factor = (age_days as f64 / 365.0).min(1.0);

    // Kind weight (some kinds are more expendable)
    let kind_weight = match kind {
        "todo" => 1.2,       // Completed/stale todos are prime candidates
        "bug" => 1.0,        // Old bugs are likely resolved
        "note" => 0.9,       // Notes may be transient
        "snippet" => 0.6,    // Snippets are often reusable
        "decision" => 0.3,   // Decisions are important context
        "preference" => 0.2, // Preferences should persist
        "pattern" => 0.2,    // Patterns are reusable
        "fact" => 0.4,       // Facts may become outdated
        "credential" => 0.1, // Credentials should persist
        _ => 0.5,
    };

    (importance_score * 0.4 + age_factor * 0.3 + kind_weight * 0.3).min(1.0)
}

/// Common English/French stopwords to skip during keyword extraction.
fn is_stopword(word: &str) -> bool {
    matches!(word,
        // English
        "the" | "this" | "that" | "with" | "from" | "have" | "been" | "will"
        | "should" | "would" | "could" | "when" | "where" | "what" | "which"
        | "their" | "there" | "they" | "them" | "then" | "than" | "these"
        | "those" | "into" | "some" | "such" | "also" | "does"
        | "done" | "each" | "just" | "like" | "make" | "made" | "more"
        | "most" | "much" | "need" | "only" | "over" | "very" | "well"
        | "about" | "after" | "again" | "being" | "other" | "using"
        // French
        | "dans" | "pour" | "avec" | "cette" | "sont" | "mais" | "plus"
        | "tout" | "tous" | "toute" | "comme" | "faire" | "fait" | "peut"
        | "sans" | "encore" | "entre" | "aussi" | "autre" | "avant"
    )
}
