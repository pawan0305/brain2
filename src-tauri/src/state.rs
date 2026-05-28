use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub id: Uuid,
    pub started_at: DateTime<Utc>,
    pub dutch: String,
    #[serde(default)]
    pub english: Option<String>,
    /// Free-form display name (kept for backward compat with old JSON files).
    #[serde(default)]
    pub speaker: Option<String>,
    /// Deepgram diarization speaker id (0, 1, 2, …). Maps to a human name
    /// via `Meeting::speaker_names` if the user labelled it.
    #[serde(default)]
    pub speaker_id: Option<u32>,
    pub is_final: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetingCost {
    /// Seconds of audio streamed to Deepgram.
    #[serde(default)]
    pub deepgram_audio_secs: f64,
    /// Anthropic input tokens consumed.
    #[serde(default)]
    pub anthropic_input_tokens: u64,
    /// Anthropic output tokens consumed.
    #[serde(default)]
    pub anthropic_output_tokens: u64,
    /// Cache-read tokens (cheaper); separate so totals stay honest.
    #[serde(default)]
    pub anthropic_cache_read_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meeting {
    pub id: Uuid,
    pub title: String,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub segments: Vec<Segment>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub summary_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub chat: Vec<ChatMessage>,
    /// User-typed freeform notes for this meeting.
    #[serde(default)]
    pub notes: String,
    /// Filter tags. Plain strings ("standup", "customer", "project-X").
    #[serde(default)]
    pub tags: Vec<String>,
    /// Mapping from Deepgram speaker_id → user-given human name.
    /// Keys are stringified u32 to keep JSON simple.
    #[serde(default)]
    pub speaker_names: HashMap<String, String>,
    /// Running cost tally.
    #[serde(default)]
    pub cost: MeetingCost,
}

impl Meeting {
    pub fn new(title: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            title,
            started_at: Utc::now(),
            ended_at: None,
            segments: vec![],
            summary: None,
            summary_updated_at: None,
            chat: vec![],
            notes: String::new(),
            tags: vec![],
            speaker_names: HashMap::new(),
            cost: MeetingCost::default(),
        }
    }

    pub fn finalized_text(&self, include_english: bool) -> String {
        let mut out = String::new();
        for seg in self.segments.iter().filter(|s| s.is_final) {
            let ts = seg.started_at.format("%H:%M:%S");
            out.push_str(&format!("[{ts}] NL: {}\n", seg.dutch.trim()));
            if include_english {
                if let Some(en) = seg.english.as_deref() {
                    out.push_str(&format!("[{ts}] EN: {}\n", en.trim()));
                }
            }
        }
        out
    }

    /// Concatenated source-language transcript, plain text — no timestamps.
    /// Fed to Claude for translate / summarize / chat. Timestamps were
    /// noise; they showed up in the copied transcript and made Claude's
    /// output less readable.
    pub fn source_text(&self) -> String {
        let mut out = String::new();
        for seg in self.segments.iter().filter(|s| s.is_final) {
            let line = seg.dutch.trim();
            if line.is_empty() { continue; }
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    /// Human-readable transcript with [HH:MM:SS] timestamps — the format
    /// the user wants for download. Use this for downloaded files (raw +
    /// cleaned/translated), NOT for prompts to the LLM (which want
    /// `source_text`).
    pub fn formatted_transcript(&self) -> String {
        let mut out = String::new();
        // Header so the file is self-describing.
        out.push_str(&format!("# {}\n", self.title));
        out.push_str(&format!(
            "# Started: {}\n",
            self.started_at.format("%Y-%m-%d %H:%M:%S %Z"),
        ));
        if let Some(end) = self.ended_at {
            out.push_str(&format!(
                "# Ended:   {}\n",
                end.format("%Y-%m-%d %H:%M:%S %Z"),
            ));
        }
        out.push('\n');

        for seg in self.segments.iter().filter(|s| s.is_final) {
            let line = seg.dutch.trim();
            if line.is_empty() { continue; }
            let ts = seg.started_at.format("%H:%M:%S");
            out.push_str(&format!("[{ts}] {line}\n"));
        }
        out
    }
}

pub struct MeetingHandle {
    pub meeting: Arc<RwLock<Meeting>>,
    pub cancel: CancellationToken,
    /// When set, audio bytes are dropped before reaching Deepgram. The audio
    /// sidecar keeps running (cheap) so resuming is instant, but Deepgram
    /// usage and Claude translation calls stop. The reconnect loop will
    /// drop the WebSocket after its idle timeout while paused; that's fine
    /// — the loop reconnects automatically when bytes start flowing again.
    pub paused: Arc<std::sync::atomic::AtomicBool>,
}

pub struct AppState {
    pub app_handle: AppHandle,
    pub data_dir: PathBuf,
    inner: RwLock<Inner>,
}

#[derive(Default)]
struct Inner {
    current: Option<Arc<MeetingHandle>>,
}

impl AppState {
    pub fn new(app_handle: AppHandle, data_dir: PathBuf) -> Self {
        Self {
            app_handle,
            data_dir,
            inner: RwLock::new(Inner::default()),
        }
    }

    pub fn meetings_dir(&self) -> PathBuf {
        self.data_dir.join("meetings")
    }

    pub fn current(&self) -> Option<Arc<MeetingHandle>> {
        self.inner.read().current.clone()
    }

    pub fn set_current(&self, handle: Arc<MeetingHandle>) {
        self.inner.write().current = Some(handle);
    }

    pub fn clear_current(&self) {
        self.inner.write().current = None;
    }

    pub fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) {
        if let Err(err) = self.app_handle.emit(event, payload) {
            tracing::warn!(?err, %event, "emit failed");
        }
    }
}
