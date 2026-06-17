//! Brain2 Brain Engine — proactive 2nd brain during meetings.
//!
//! While a meeting is being transcribed, the Brain engine runs in the
//! background, analyzing the conversation in real time and producing:
//!
//! 1. **Action items** — detected from phrases like "I'll do that",
//!    "let's schedule", "send him an email"
//! 2. **Decisions** — logged when the group reaches consensus
//! 3. **Cross-meeting memory** — searches past meetings for related
//!    threads and surfaces relevant context
//! 4. **Background agent** — can draft emails, create docs, look things
//!    up, all triggered by meeting context
//!
//! The engine runs on a debounced timer — it waits for a pause in
//! speech, then processes the latest transcript chunk.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::settings;

// ── Types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub id: Uuid,
    pub text: String,
    pub assignee: Option<String>,
    pub detected_at: DateTime<Utc>,
    pub meeting_id: Uuid,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: Uuid,
    pub text: String,
    pub context: String,
    pub detected_at: DateTime<Utc>,
    pub meeting_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryThread {
    pub id: Uuid,
    pub title: String,
    pub related_meetings: Vec<Uuid>,
    pub summary: String,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainEvent {
    pub kind: String, // "action_item" | "decision" | "context_recall" | "suggestion"
    pub content: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainStatus {
    pub action_items: Vec<ActionItem>,
    pub decisions: Vec<Decision>,
    pub threads: Vec<MemoryThread>,
    pub events: Vec<BrainEvent>,
    pub enabled: bool,
}

pub struct BrainEngine {
    app_handle: AppHandle,
    data_dir: PathBuf,
    inner: RwLock<BrainInner>,
}

struct BrainInner {
    action_items: Vec<ActionItem>,
    decisions: Vec<Decision>,
    threads: Vec<MemoryThread>,
    events: Vec<BrainEvent>,
    enabled: bool,
    last_processed_segment: usize,
}

impl BrainEngine {
    pub fn new(app_handle: AppHandle, data_dir: PathBuf) -> Self {
        let brain_dir = data_dir.join("brain");
        let _ = fs::create_dir_all(&brain_dir);

        let mut engine = Self {
            app_handle,
            data_dir: brain_dir.clone(),
            inner: RwLock::new(BrainInner {
                action_items: vec![],
                decisions: vec![],
                threads: vec![],
                events: vec![],
                enabled: true,
                last_processed_segment: 0,
            }),
        };

        engine.load_state();
        engine
    }

    pub fn brain_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    /// Process new transcript content since last check. Called periodically
    /// while a meeting is running.
    pub async fn process_new_content(
        &self,
        meeting_id: Uuid,
        meeting_title: &str,
        transcript_text: &str,
        segment_count: usize,
        api_key: &str,
    ) -> Result<Vec<BrainEvent>> {
        let last = {
            let inner = self.inner.read();
            if !inner.enabled {
                return Ok(vec![]);
            }
            inner.last_processed_segment
        };

        // Only process if there are new segments
        if segment_count <= last || transcript_text.trim().is_empty() {
            return Ok(vec![]);
        }

        // Get the new content only — the segments added since `last`. The
        // caller passes segment_count == number of transcript lines, so the
        // already-processed text is the first `last` lines and the new text
        // is everything after it.
        let new_content = if last == 0 {
            transcript_text.to_string()
        } else {
            let lines: Vec<&str> = transcript_text.lines().collect();
            let start = last.min(lines.len());
            lines[start..].join("\n")
        };

        let mut events = vec![];

        // 1. Detect action items
        let actions = detect_action_items(api_key, &new_content).await?;
        for action in actions {
            let assignee = extract_assignee(&action);
            let item = ActionItem {
                id: Uuid::new_v4(),
                text: action,
                assignee,
                detected_at: Utc::now(),
                meeting_id,
                done: false,
            };
            let event = BrainEvent {
                kind: "action_item".into(),
                content: item.text.clone(),
                at: Utc::now(),
            };
            self.inner.write().action_items.push(item);
            self.inner.write().events.push(event.clone());
            events.push(event);
        }

        // 2. Detect decisions
        let decisions = detect_decisions(api_key, &new_content).await?;
        for decision in decisions {
            let item = Decision {
                id: Uuid::new_v4(),
                text: decision.clone(),
                context: meeting_title.to_string(),
                detected_at: Utc::now(),
                meeting_id,
            };
            let event = BrainEvent {
                kind: "decision".into(),
                content: decision,
                at: Utc::now(),
            };
            self.inner.write().decisions.push(item);
            self.inner.write().events.push(event.clone());
            events.push(event);
        }

        // 3. Cross-meeting memory — search past meetings for related context
        if segment_count % 5 == 0 {
            // Every ~5 segments, do a context recall
            if let Ok(recall) = recall_context(api_key, meeting_title, &new_content).await {
                if !recall.is_empty() {
                    let event = BrainEvent {
                        kind: "context_recall".into(),
                        content: recall,
                        at: Utc::now(),
                    };
                    self.inner.write().events.push(event.clone());
                    events.push(event);
                }
            }
        }

        // Update last processed
        {
            let mut inner = self.inner.write();
            inner.last_processed_segment = segment_count;
        }

        self.save_state();
        self.emit_status();
        Ok(events)
    }

    /// Generate a full meeting wrap-up: consolidate actions, decisions,
    /// and key topics from the complete transcript.
    pub async fn wrap_up(
        &self,
        meeting_id: Uuid,
        meeting_title: &str,
        full_transcript: &str,
        api_key: &str,
    ) -> Result<String> {
        let actions = self.get_actions_for_meeting(meeting_id);
        let decisions = self.get_decisions_for_meeting(meeting_id);

        let actions_text = actions
            .iter()
            .map(|a| format!("- {} {}", if a.done { "✅" } else { "⏳" }, a.text))
            .collect::<Vec<_>>()
            .join("\n");

        let decisions_text = decisions
            .iter()
            .map(|d| format!("- {}", d.text))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "You are a meeting assistant wrapping up a meeting. Based on the transcript \
            and the detected items below, produce a clean wrap-up with these sections:\n\n\
            ## Key Topics\n(summarize the main themes discussed)\n\n\
            ## Decisions Made\n{decisions_text}\n(organize and add any that were missed)\n\n\
            ## Action Items\n{actions_text}\n(organize and add any that were missed)\n\n\
            ## Next Steps\n(what should happen next)\n\n\
            MEETING: {meeting_title}\n\nTRANSCRIPT:\n{full_transcript}"
        );

        let text = crate::agent::run_text(
            crate::agent::current_backend(),
            &crate::agent::persona(),
            "You are a precise meeting assistant. Be concise and actionable.",
            &prompt,
            api_key,
        )
        .await?;

        Ok(text)
    }

    // ── Queries ──────────────────────────────

    pub fn get_actions_for_meeting(&self, meeting_id: Uuid) -> Vec<ActionItem> {
        self.inner
            .read()
            .action_items
            .iter()
            .filter(|a| a.meeting_id == meeting_id)
            .cloned()
            .collect()
    }

    pub fn get_decisions_for_meeting(&self, meeting_id: Uuid) -> Vec<Decision> {
        self.inner
            .read()
            .decisions
            .iter()
            .filter(|d| d.meeting_id == meeting_id)
            .cloned()
            .collect()
    }

    pub fn get_all_actions(&self) -> Vec<ActionItem> {
        self.inner.read().action_items.clone()
    }

    pub fn mark_action_done(&self, id: Uuid) -> Result<()> {
        let mut inner = self.inner.write();
        if let Some(item) = inner.action_items.iter_mut().find(|a| a.id == id) {
            item.done = true;
        }
        self.save_state();
        self.emit_status();
        Ok(())
    }

    pub fn status(&self) -> BrainStatus {
        let inner = self.inner.read();
        BrainStatus {
            action_items: inner.action_items.clone(),
            decisions: inner.decisions.clone(),
            threads: inner.threads.clone(),
            events: inner.events.clone(),
            enabled: inner.enabled,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.inner.write().enabled = enabled;
        self.save_state();
        self.emit_status();
    }

    // ── Persistence ──────────────────────────

    fn save_state(&self) {
        let inner = self.inner.read();
        let state = serde_json::json!({
            "action_items": inner.action_items,
            "decisions": inner.decisions,
            "threads": inner.threads,
            "events": inner.events,
            "enabled": inner.enabled,
            "last_processed_segment": inner.last_processed_segment,
        });
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = fs::write(self.brain_dir().join("brain_state.json"), json);
        }
    }

    fn load_state(&mut self) {
        let path = self.brain_dir().join("brain_state.json");
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(&data) {
                let mut inner = self.inner.write();
                if let Some(items) = state["action_items"].as_array() {
                    if let Ok(parsed) = serde_json::from_value(serde_json::Value::Array(items.clone())) {
                        inner.action_items = parsed;
                    }
                }
                if let Some(items) = state["decisions"].as_array() {
                    if let Ok(parsed) = serde_json::from_value(serde_json::Value::Array(items.clone())) {
                        inner.decisions = parsed;
                    }
                }
                if let Some(t) = state["threads"].as_array() {
                    if let Ok(parsed) = serde_json::from_value(serde_json::Value::Array(t.clone())) {
                        inner.threads = parsed;
                    }
                }
                if let Some(e) = state["events"].as_array() {
                    if let Ok(parsed) = serde_json::from_value(serde_json::Value::Array(e.clone())) {
                        inner.events = parsed;
                    }
                }
                inner.enabled = state["enabled"].as_bool().unwrap_or(true);
                inner.last_processed_segment =
                    state["last_processed_segment"].as_u64().unwrap_or(0) as usize;
            }
        }
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app_handle.emit("brain:status", status);
    }
}

// ── Detection helpers (Claude-powered) ───────

async fn detect_action_items(api_key: &str, text: &str) -> Result<Vec<String>> {
    call_claude_extract(
        api_key,
        "Extract action items from this meeting transcript. Return ONLY a JSON array of strings, \
         one per action item. Include who is responsible if mentioned. \
         Example: [\"Pawan will send the deck to Bob by Friday\", \"Martijn to review the EBS quote\"]. \
         If there are no action items, return [].",
        text,
    )
    .await
}

async fn detect_decisions(api_key: &str, text: &str) -> Result<Vec<String>> {
    call_claude_extract(
        api_key,
        "Extract decisions made in this meeting transcript. Return ONLY a JSON array of strings. \
         Example: [\"Team agreed to use Rust for the new service\", \"Budget approved for Q3\"]. \
         If no decisions were made, return [].",
        text,
    )
    .await
}

async fn recall_context(api_key: &str, meeting_title: &str, text: &str) -> Result<String> {
    let prompt = format!(
        "You are a context-aware meeting assistant. Based on this meeting transcript excerpt, \
         identify any references to past discussions, prior decisions, or recurring themes. \
         Respond in 1-3 sentences. If nothing references the past, respond with an empty string.\n\n\
         MEETING: {meeting_title}\n\nTRANSCRIPT:\n{text}"
    );

    let out = crate::agent::run_text(
        crate::agent::current_backend(),
        &crate::agent::persona(),
        "Be concise. Return an empty string if nothing is relevant.",
        &prompt,
        api_key,
    )
    .await?;
    Ok(out.trim().to_string())
}

async fn call_claude_extract(api_key: &str, instruction: &str, text: &str) -> Result<Vec<String>> {
    let raw = crate::agent::run_text(
        crate::agent::current_backend(),
        &crate::agent::persona(),
        instruction,
        text,
        api_key,
    )
    .await?;

    // Try to parse as JSON array
    if let Ok(items) = serde_json::from_str::<Vec<String>>(&raw) {
        return Ok(items);
    }

    // Try to extract from code block
    if let Some(start) = raw.find('[') {
        if let Some(end) = raw.rfind(']') {
            let slice = &raw[start..=end];
            if let Ok(items) = serde_json::from_str::<Vec<String>>(slice) {
                return Ok(items);
            }
        }
    }

    Ok(vec![])
}

fn extract_assignee(text: &str) -> Option<String> {
    // Simple heuristic: "NAME will..." or "NAME to..."
    let lower = text.to_lowercase();
    let patterns = [" will ", " to ", " should ", " needs to "];
    for pat in &patterns {
        if let Some(pos) = lower.find(pat) {
            let before = &text[..pos].trim();
            // Take the last word(s) before the pattern
            let words: Vec<&str> = before.split_whitespace().collect();
            if !words.is_empty() {
                let name = words[words.len().saturating_sub(2)..].join(" ");
                if name.len() > 2 && name.chars().next().map_or(false, |c| c.is_uppercase()) {
                    return Some(name);
                }
            }
        }
    }
    None
}

// ── Tauri commands ───────────────────────────

use tauri::State;

#[tauri::command]
pub async fn brain_status(
    brain: State<'_, Arc<BrainEngine>>,
) -> Result<BrainStatus, String> {
    Ok(brain.status())
}

#[tauri::command]
pub async fn brain_toggle(
    enabled: bool,
    brain: State<'_, Arc<BrainEngine>>,
) -> Result<BrainStatus, String> {
    brain.set_enabled(enabled);
    Ok(brain.status())
}

#[tauri::command]
pub async fn brain_mark_action_done(
    id: Uuid,
    brain: State<'_, Arc<BrainEngine>>,
) -> Result<BrainStatus, String> {
    brain.mark_action_done(id).map_err(|e| e.to_string())?;
    Ok(brain.status())
}

#[tauri::command]
pub async fn brain_wrap_up(
    meeting_id: Uuid,
    meeting_title: String,
    full_transcript: String,
    brain: State<'_, Arc<BrainEngine>>,
) -> Result<String, String> {
    let api_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;
    // Use the managed engine so the wrap-up sees the action items and
    // decisions already detected for this meeting.
    brain
        .wrap_up(meeting_id, &meeting_title, &full_transcript, &api_key)
        .await
        .map_err(|e| e.to_string())
}
