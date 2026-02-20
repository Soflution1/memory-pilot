use notify::{Watcher, RecursiveMode, Event, EventKind};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use std::path::PathBuf;
use chrono::Utc;

pub struct FileWatcherState {
    pub recent_changes: VecDeque<FileChange>,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub filename: String,
    pub timestamp: String,
}

impl FileWatcherState {
    pub fn new() -> Self {
        Self {
            recent_changes: VecDeque::with_capacity(20),
        }
    }

    pub fn push(&mut self, change: FileChange) {
        if self.recent_changes.len() >= 20 {
            self.recent_changes.pop_front();
        }
        self.recent_changes.push_back(change);
    }

    /// Keywords from recent file changes for search boosting.
    pub fn get_boost_keywords(&self) -> Vec<String> {
        let mut words = Vec::new();
        for c in &self.recent_changes {
            let stem = c.filename.split('.').next().unwrap_or(&c.filename);
            let mut current_word = String::new();
            for ch in stem.chars() {
                if ch.is_alphanumeric() {
                    if ch.is_uppercase() && !current_word.is_empty() {
                        words.push(current_word.clone());
                        current_word.clear();
                    }
                    current_word.push(ch);
                } else if !current_word.is_empty() {
                    words.push(current_word.clone());
                    current_word.clear();
                }
            }
            if !current_word.is_empty() {
                words.push(current_word);
            }
        }
        words
    }
}

pub fn start_watcher(dir: &str) -> Option<Arc<Mutex<FileWatcherState>>> {
    let state = Arc::new(Mutex::new(FileWatcherState::new()));
    let state_clone = state.clone();
    let dir_path = PathBuf::from(dir);

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        }) {
            Ok(w) => w,
            Err(_) => return,
        };
        
        if watcher.watch(&dir_path, RecursiveMode::Recursive).is_err() {
            return;
        }

        for event in rx {
            if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) { continue; }
            for path in &event.paths {
                let path_str = path.to_string_lossy();
                // Skip .git, node_modules, target, hidden files
                if path_str.contains("/.") || path_str.contains("/node_modules/")
                    || path_str.contains("/target/") { continue; }
                let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                if filename.is_empty() { continue; }
                
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if !["rs", "ts", "svelte", "py", "js", "go", "tsx", "jsx", "md"].contains(&ext) {
                        continue;
                    }
                }
                
                if let Ok(mut s) = state_clone.lock() {
                    s.push(FileChange {
                        path: path_str.to_string(),
                        filename,
                        timestamp: Utc::now().to_rfc3339(),
                    });
                }
            }
        }
    });

    Some(state)
}
