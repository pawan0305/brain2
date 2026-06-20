//! Retrieval from the user's **Knowledge folder** — a lean, agentic search
//! that replaces the old gbrain vector DB.
//!
//! The Knowledge folder is a collection of markdown files on disk (OneDrive-
//! synced). Instead of pre-computing embeddings and maintaining a database,
//! we do a fast keyword search across all files and return the most relevant
//! ones. The LLM (Claude/Hermes) does the semantic understanding — this
//! module just finds the right files.
//!
//! Everything runs on-device: a simple directory walk + case-insensitive text
//! match. No embedding server, no database, no WSL dependency.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Maximum number of files to return from a search.
const MAX_RESULTS: usize = 5;
/// Maximum chars of a file to include in the context block.
const MAX_FILE_CHARS: usize = 4000;
/// Maximum total chars in the returned context block.
const MAX_CONTEXT_CHARS: usize = 12000;

/// Search the Knowledge folder for files matching `question`, then return
/// a prompt-ready context block with the most relevant file contents.
///
/// Returns an empty string if no matches are found. Errors are logged and
/// treated as "no knowledge" by callers.
pub async fn retrieve(question: &str) -> Result<String> {
    let kdir = crate::settings::read_knowledge_dir();
    let kdir_path = PathBuf::from(&kdir);
    if !kdir_path.exists() {
        return Ok(String::new());
    }

    // Extract keywords from the question (skip common stop words)
    let keywords = extract_keywords(question);
    if keywords.is_empty() {
        return Ok(String::new());
    }

    // Walk the Knowledge folder and score each markdown file
    let files = walk_markdown(&kdir_path);
    let scored: Vec<(usize, PathBuf)> = files
        .iter()
        .filter_map(|path| {
            let content = fs::read_to_string(path).ok()?;
            let score = score_file(&content, &keywords);
            if score > 0 {
                Some((score, path.clone()))
            } else {
                None
            }
        })
        .collect();

    if scored.is_empty() {
        return Ok(String::new());
    }

    // Sort by score descending, take top N
    let mut ranked: Vec<(usize, PathBuf)> = scored;
    ranked.sort_by(|a, b| b.0.cmp(&a.0));
    ranked.truncate(MAX_RESULTS);

    // Build the context block
    let mut context = String::new();
    context.push_str("Top matches:\n");
    for (i, (score, path)) in ranked.iter().enumerate() {
        let rel = path.strip_prefix(&kdir_path).unwrap_or(path).to_string_lossy();
        context.push_str(&format!("[{}] {} (score: {})\n", i + 1, rel, score));
    }
    context.push_str("\nFull text of the most relevant pages:\n\n");

    let mut total_chars = 0;
    for (_, path) in &ranked {
        let rel = path.strip_prefix(&kdir_path).unwrap_or(path).to_string_lossy();
        let content = fs::read_to_string(path).unwrap_or_default();
        let truncated = if content.len() > MAX_FILE_CHARS {
            &content[..MAX_FILE_CHARS.min(content.len())]
        } else {
            &content
        };
        context.push_str(&format!("=== {} ===\n{}\n\n", rel, truncated));
        total_chars += truncated.len();
        if total_chars >= MAX_CONTEXT_CHARS {
            break;
        }
    }

    Ok(context)
}

/// Walk a directory recursively and return all .md files.
fn walk_markdown(dir: &Path) -> Vec<PathBuf> {
    let mut results = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if let Ok(entries) = fs::read_dir(&current) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip hidden directories (like .git, _processed)
                    if let Some(name) = path.file_name() {
                        if !name.to_string_lossy().starts_with('.') && !name.to_string_lossy().starts_with('_') {
                            stack.push(path);
                        }
                    }
                } else if path.extension().map_or(false, |ext| ext == "md") {
                    results.push(path);
                }
            }
        }
    }
    results
}

/// Extract meaningful keywords from a question — skip common stop words.
fn extract_keywords(question: &str) -> Vec<String> {
    let stop_words: &[&str] = &[
        "the", "a", "an", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "must", "shall", "can", "need", "dare",
        "ought", "used", "to", "of", "in", "for", "on", "with", "at", "by",
        "from", "as", "into", "through", "during", "before", "after", "above",
        "below", "between", "under", "further", "then", "once", "here",
        "there", "when", "where", "why", "how", "all", "each", "few", "more",
        "most", "other", "some", "such", "no", "nor", "not", "only", "own",
        "same", "so", "than", "too", "very", "just", "about", "also", "what",
        "which", "who", "whom", "this", "that", "these", "those", "i", "me",
        "my", "we", "us", "our", "you", "your", "he", "him", "his", "she",
        "her", "it", "its", "they", "them", "their", "and", "or", "but",
        "if", "while", "because", "until", "up", "down", "out", "off",
        "over", "under", "again", "any", "both", "each",
    ];
    question
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| w.len() > 2 && !stop_words.contains(&w.as_str()))
        .collect()
}

/// Score a file's relevance to the keywords — count keyword occurrences
/// (case-insensitive), with a bonus for title/header matches.
fn score_file(content: &str, keywords: &[String]) -> usize {
    let lower = content.to_lowercase();
    let mut score = 0;
    for kw in keywords {
        // Count occurrences of the keyword
        let count = lower.matches(kw.as_str()).count();
        score += count;

        // Bonus: keyword appears in a header line (starts with #)
        for line in lower.lines() {
            if line.starts_with('#') && line.contains(kw.as_str()) {
                score += 3;
            }
        }

        // Bonus: keyword appears in the first 200 chars (likely title/frontmatter)
        if lower.len() > 200 && lower[..200].contains(kw.as_str()) {
            score += 2;
        }
    }
    score
}

/// Check whether the Knowledge folder is accessible — used by the supervisor
/// for the stack health strip.
pub fn check_health() -> (bool, String) {
    let kdir = crate::settings::read_knowledge_dir();
    let path = PathBuf::from(&kdir);
    if !path.exists() {
        return (false, format!("Knowledge folder not found: {}", kdir));
    }
    // Count markdown files
    let files = walk_markdown(&path);
    let count = files.len();
    if count == 0 {
        return (true, "Knowledge folder empty".to_string());
    }
    (true, format!("{} markdown files", count))
}
