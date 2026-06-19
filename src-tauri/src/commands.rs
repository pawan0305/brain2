use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Manager, State};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::anthropic::ChatStreamEvent;
use crate::brain::BrainEngine;
use crate::factory::FactoryConnector;
use crate::llm::LlmClient;
use crate::audio;
use crate::deepgram::{self, DeepgramConfig, DeepgramEvent};
use crate::settings::{self, SettingsView};
use crate::state::{AppState, ChatMessage, Meeting, MeetingHandle, Segment};
use crate::storage::{self, MeetingSummaryRow};

#[derive(Serialize)]
pub struct AskHandle {
    pub stream_id: Uuid,
}

// ------- settings -------

#[tauri::command]
pub async fn get_settings() -> Result<SettingsView, String> {
    settings::settings_view().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_api_keys(
    deepgram: Option<String>,
    anthropic: Option<String>,
) -> Result<SettingsView, String> {
    settings::write_keys(deepgram.as_deref(), anthropic.as_deref())
        .map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Toggle per-chunk translation on/off. Persists to keys.json so it survives
/// restarts. Reads on every Final segment in handle_dg_event so the toggle
/// takes effect mid-meeting without restarting capture.
#[tauri::command]
pub async fn set_translate_enabled(enabled: bool) -> Result<SettingsView, String> {
    settings::set_translate_enabled(enabled).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Toggle microphone capture. When off, only system audio (WASAPI loopback)
/// is transcribed — use this on speakers, where the mic also picks up the
/// system audio from the speakers and the transcript comes out doubled. On
/// headphones, leave it on to capture your own voice too. Applies on the next
/// meeting (capture threads are spawned at meeting start).
#[tauri::command]
pub async fn set_capture_mic(enabled: bool) -> Result<SettingsView, String> {
    settings::set_capture_mic(enabled).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Show / hide / change the subtitle overlay window. Modes: "off", "dual",
/// "en". Persists across restarts. The overlay webview listens to the
/// `overlay:mode` event for live mode switching while it's already visible.
#[tauri::command]
pub async fn set_overlay_mode(mode: String, app: AppHandle) -> Result<SettingsView, String> {
    settings::set_overlay_mode(&mode).map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window("overlay") {
        if mode == "off" {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_always_on_top(true);
            // Visible on every macOS Space, so the subtitles follow you when
            // you switch desktops. No-op on platforms that don't support it.
            #[cfg(target_os = "macos")]
            {
                let _ = win.set_visible_on_all_workspaces(true);
            }
            // Re-apply the persisted lock state every time we show, in case
            // it was toggled while hidden.
            let locked = settings::read_overlay_locked();
            let _ = win.set_ignore_cursor_events(locked);
        }
    }
    use tauri::Emitter;
    let _ = app.emit("overlay:mode", json!({ "mode": mode }));
    settings::settings_view().map_err(|e| e.to_string())
}

/// Save the user's custom vocabulary list (fed to Deepgram as keyterms).
#[tauri::command]
pub async fn set_vocab(words: Vec<String>) -> Result<SettingsView, String> {
    settings::set_keywords(words).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Pick the LLM backend. "anthropic" or "openai" — the latter routes
/// translation, summary, and chat through any OpenAI-compatible endpoint
/// (OpenAI itself, Ollama, LM Studio, vLLM, OpenRouter, etc.).
#[tauri::command]
pub async fn set_llm_provider(provider: String) -> Result<SettingsView, String> {
    settings::set_llm_provider(&provider).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Persist the OpenAI-compatible endpoint config. Any field passed as
/// `None` is left untouched. Empty string for `api_key` clears the key.
#[tauri::command]
pub async fn set_openai_config(
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<SettingsView, String> {
    settings::set_openai_config(
        api_key.as_deref(),
        base_url.as_deref(),
        model.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Set the Deepgram source-language code ("multi" = auto-detect, "nl" =
/// Dutch, "nl-BE" = Flemish, "en", "de", …). Locking to a single language
/// is more accurate than multi when you know what's being spoken. Applies
/// when the next meeting's Deepgram connection opens.
#[tauri::command]
pub async fn set_source_language(code: String) -> Result<SettingsView, String> {
    settings::set_source_language(&code).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Set the target language Claude uses for translation, summary, and chat.
/// Source language is auto-detected by Deepgram. Takes effect on the next
/// translation / summary / chat call (no restart needed).
#[tauri::command]
pub async fn set_target_language(
    language: String,
    app: AppHandle,
) -> Result<SettingsView, String> {
    settings::set_target_language(&language).map_err(|e| e.to_string())?;
    let resolved = settings::read_target_language();
    use tauri::Emitter;
    let _ = app.emit(
        "overlay:target_language",
        json!({ "target_language": resolved }),
    );
    settings::settings_view().map_err(|e| e.to_string())
}

/// Persist the overlay window position + size so it doesn't reset on restart.
#[tauri::command]
pub async fn save_overlay_geometry(x: i32, y: i32, w: u32, h: u32) -> Result<(), String> {
    settings::set_overlay_geometry(x, y, w, h).map_err(|e| e.to_string())
}

/// Update the live meeting's notes (string).
#[tauri::command]
pub async fn set_meeting_notes(
    id: Option<Uuid>,
    notes: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    if let Some(handle) = state.current() {
        let live_id = handle.meeting.read().id;
        if id.map(|i| i == live_id).unwrap_or(true) {
            handle.meeting.write().notes = notes;
            let snap = handle.meeting.read().clone();
            state.emit("meeting:update", snap);
            return Ok(());
        }
    }
    let Some(meeting_id) = id else { return Err("no meeting".into()) };
    let dir = state.meetings_dir();
    let state_clone = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<crate::state::Meeting> {
        let mut m = storage::load_meeting(&dir, meeting_id)?;
        m.notes = notes;
        storage::save_meeting(&dir, &m)?;
        Ok(m)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
    .map(|m| state_clone.emit("meeting:update", m))
}

/// Update tag list on the live or a historical meeting.
#[tauri::command]
pub async fn set_meeting_tags(
    id: Option<Uuid>,
    tags: Vec<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let tags: Vec<String> = tags
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(handle) = state.current() {
        let live_id = handle.meeting.read().id;
        if id.map(|i| i == live_id).unwrap_or(true) {
            handle.meeting.write().tags = tags;
            let snap = handle.meeting.read().clone();
            state.emit("meeting:update", snap);
            return Ok(());
        }
    }
    let Some(meeting_id) = id else { return Err("no meeting".into()) };
    let dir = state.meetings_dir();
    let state_clone = state.inner().clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<crate::state::Meeting> {
        let mut m = storage::load_meeting(&dir, meeting_id)?;
        m.tags = tags;
        storage::save_meeting(&dir, &m)?;
        Ok(m)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
    .map(|m| state_clone.emit("meeting:update", m))
}

/// Adjust the subtitle font size in pixels.
#[tauri::command]
pub async fn set_overlay_font_size(
    size: u32,
    app: AppHandle,
) -> Result<SettingsView, String> {
    settings::set_overlay_font_size(size).map_err(|e| e.to_string())?;
    let view = settings::settings_view().map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit(
        "overlay:settings",
        json!({ "font_size": view.overlay_font_size, "locked": view.overlay_locked }),
    );
    Ok(view)
}

/// Lock / unlock the overlay. When locked the overlay is click-through —
/// every click goes to whatever is behind it (Teams, browser, etc). When
/// unlocked the user can grab and drag/resize it.
#[tauri::command]
pub async fn set_overlay_locked(
    locked: bool,
    app: AppHandle,
) -> Result<SettingsView, String> {
    settings::set_overlay_locked(locked).map_err(|e| e.to_string())?;
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_ignore_cursor_events(locked);
    }
    let view = settings::settings_view().map_err(|e| e.to_string())?;
    use tauri::Emitter;
    let _ = app.emit(
        "overlay:settings",
        json!({ "font_size": view.overlay_font_size, "locked": view.overlay_locked }),
    );
    Ok(view)
}

/// Pick the agent harness that drives Brain2's reasoning (Brain engine +
/// Forge): "direct" (Anthropic Haiku), "claude_code" (Claude Code CLI), or
/// "hermes" (Hermes agent in WSL). The agent IS Brain2; the harness is a
/// swappable shell around the shared agent-prompts/BRAIN2.md persona.
#[tauri::command]
pub async fn set_agent_backend(backend: String, app: AppHandle) -> Result<SettingsView, String> {
    settings::set_agent_backend(&backend).map_err(|e| e.to_string())?;
    // Switching onto an agent backend? Warm it now so it's hot the moment the
    // user opens "Ask the meeting", not on first use.
    spawn_warm_up(app);
    settings::settings_view().map_err(|e| e.to_string())
}

/// Kick off a background warm-up of the selected agent backend so the first
/// "Ask the meeting" query is fast (see `agent::warm_up`). No-op for the Direct
/// backend. Progress is reported to the UI via the `agent:status` event with a
/// `state` of `warming`, `ready`, or `error`.
pub fn spawn_warm_up(app: AppHandle) {
    use tauri::Emitter;
    let backend = crate::agent::current_backend();
    if backend == crate::agent::AgentBackend::Direct {
        return;
    }
    // The CLI may be authenticated via its own login, so an absent API key is
    // not fatal — pass whatever we have.
    let key = settings::require_anthropic().unwrap_or_default();
    tauri::async_runtime::spawn(async move {
        let root = std::path::PathBuf::from(
            std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()),
        );
        let _ = app.emit("agent:status", json!({ "state": "warming" }));
        match crate::agent::warm_up(&root, &key).await {
            Ok(()) => {
                let _ = app.emit("agent:status", json!({ "state": "ready" }));
            }
            Err(e) => {
                let _ = app.emit(
                    "agent:status",
                    json!({ "state": "error", "error": e.to_string() }),
                );
            }
        }
    });
}

/// Manually (re-)warm the agent backend on demand from the frontend.
#[tauri::command]
pub async fn warm_agent(app: AppHandle) -> Result<(), String> {
    spawn_warm_up(app);
    Ok(())
}

/// Master switch for the brain feeder — the continuous gbrain populator
/// (meeting + project-work distillation). The user's pause control.
#[tauri::command]
pub async fn set_brain_feed_enabled(enabled: bool) -> Result<SettingsView, String> {
    settings::set_brain_feed_enabled(enabled).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Set the repos the project-work feed watches (read-only on them).
#[tauri::command]
pub async fn set_brain_feed_repos(repos: Vec<String>) -> Result<SettingsView, String> {
    settings::set_brain_feed_repos(repos).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Configure the Hermes backend's provider/model — the knob for pointing
/// Brain2's brain at a local LLM (e.g. provider "ollama"). A field passed as
/// None is left unchanged.
#[tauri::command]
pub async fn set_hermes_config(
    provider: Option<String>,
    model: Option<String>,
) -> Result<SettingsView, String> {
    settings::set_hermes_config(provider.as_deref(), model.as_deref())
        .map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Set the model used by the Claude Code backend (`claude --model`, e.g.
/// "haiku", "sonnet", or a full model id).
#[tauri::command]
pub async fn set_claude_model(model: String) -> Result<SettingsView, String> {
    settings::set_claude_model(&model).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Pick the speech-to-text backend: "deepgram" (cloud, low-latency) or
/// "local_whisper" (on-device whisper.cpp — requires a `local-stt` build).
#[tauri::command]
pub async fn set_stt_backend(backend: String) -> Result<SettingsView, String> {
    settings::set_stt_backend(&backend).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Choose the local Whisper model (manifest key, e.g. "large-v3-q5_0").
#[tauri::command]
pub async fn set_whisper_model(model: String) -> Result<SettingsView, String> {
    settings::set_whisper_model(&model).map_err(|e| e.to_string())?;
    settings::settings_view().map_err(|e| e.to_string())
}

/// Download a local Whisper model to app-data; returns its path.
#[tauri::command]
pub async fn download_model(
    name: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let dir = state.data_dir.clone();
    crate::models::ensure_whisper_model(&dir, &name)
        .await
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

/// List local Whisper models + whether each is already downloaded.
#[tauri::command]
pub async fn list_local_models(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<crate::models::ModelInfo>, String> {
    Ok(crate::models::list_models(&state.data_dir))
}

// ------- meetings -------

#[tauri::command]
pub async fn start_meeting(
    title: Option<String>,
    state: State<'_, Arc<AppState>>,
    brain: State<'_, Arc<BrainEngine>>,
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<Meeting, String> {
    if state.current().is_some() {
        return Err("a meeting is already running".into());
    }
    // Deepgram key is only required for the cloud STT backend; local Whisper
    // needs no key.
    let dg_key = if settings::read_stt_backend() == "local_whisper" {
        String::new()
    } else {
        settings::require_deepgram().map_err(|e| e.to_string())?
    };
    let an_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;

    let title = title.unwrap_or_else(|| default_title());
    let meeting = Meeting::new(title);
    let cancel = CancellationToken::new();
    let handle = Arc::new(MeetingHandle {
        meeting: Arc::new(RwLock::new(meeting.clone())),
        cancel: cancel.clone(),
        paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    state.set_current(handle.clone());

    let app_state = state.inner().clone();
    let brain = brain.inner().clone();
    let factory = factory.inner().clone();
    tokio::spawn(async move {
        if let Err(err) =
            run_meeting(app_state.clone(), handle.clone(), brain, factory, dg_key, an_key).await
        {
            tracing::error!(?err, "meeting loop failed");
            app_state.emit(
                "error",
                json!({ "message": format!("meeting failed: {err}") }),
            );
            // make sure we clean up even on failure
            handle.cancel.cancel();
            let final_meeting = handle.meeting.read().clone();
            app_state.emit("meeting:stopped", final_meeting);
            app_state.clear_current();
        }
    });

    state.emit("meeting:started", meeting.clone());
    Ok(meeting)
}

#[tauri::command]
pub async fn stop_meeting(state: State<'_, Arc<AppState>>) -> Result<Meeting, String> {
    let Some(handle) = state.current() else {
        return Err("no meeting in progress".into());
    };
    handle.cancel.cancel();
    // Wait briefly so the loop can flush + save.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let m = handle.meeting.read().clone();
    Ok(m)
}

#[tauri::command]
pub async fn current_meeting(state: State<'_, Arc<AppState>>) -> Result<Option<Meeting>, String> {
    Ok(state.current().map(|h| h.meeting.read().clone()))
}

/// Pause / resume the live meeting. While paused, audio bytes are dropped
/// before reaching Deepgram so DG seconds + Anthropic tokens stop accruing.
/// The Swift audio sidecar keeps running (negligible CPU), so resuming is
/// instantaneous — no permission re-prompts, no warm-up.
#[tauri::command]
pub async fn set_paused(
    paused: bool,
    state: State<'_, Arc<AppState>>,
) -> Result<bool, String> {
    let Some(handle) = state.current() else {
        return Err("no meeting in progress".into());
    };
    handle
        .paused
        .store(paused, std::sync::atomic::Ordering::Relaxed);
    state.emit(
        "meeting:paused",
        json!({ "paused": paused }),
    );
    Ok(paused)
}

/// Read whether the live meeting is currently paused. Returns false when no
/// meeting is in progress.
#[tauri::command]
pub async fn is_paused(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state
        .current()
        .map(|h| h.paused.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(false))
}

#[tauri::command]
pub async fn set_meeting_title(
    title: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let Some(handle) = state.current() else {
        return Err("no meeting in progress".into());
    };
    {
        let mut m = handle.meeting.write();
        m.title = title;
    }
    let snapshot = handle.meeting.read().clone();
    state.emit("meeting:update", snapshot);
    Ok(())
}

#[tauri::command]
pub async fn list_meetings(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<MeetingSummaryRow>, String> {
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || storage::list_meetings(&dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn load_meeting(
    id: Uuid,
    state: State<'_, Arc<AppState>>,
) -> Result<Meeting, String> {
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || storage::load_meeting(&dir, id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_meeting(
    id: Uuid,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || storage::delete_meeting(&dir, id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Translate the full transcript of either the running meeting (id=None) or
/// a historical meeting in one shot — for the "Copy EN" button. Producing
/// one cohesive translation reads much better than concatenating the live
/// per-chunk translations.
#[tauri::command]
pub async fn export_english_transcript(
    id: Option<Uuid>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let an_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;
    let claude = LlmClient::from_settings(an_key, settings::read_target_language());

    let transcript = if let Some(meeting_id) = id {
        // Prefer the live meeting if the id matches; otherwise load from disk.
        if let Some(handle) = state.current() {
            if handle.meeting.read().id == meeting_id {
                handle.meeting.read().source_text()
            } else {
                let dir = state.meetings_dir();
                let m = tokio::task::spawn_blocking(move || storage::load_meeting(&dir, meeting_id))
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| e.to_string())?;
                m.source_text()
            }
        } else {
            let dir = state.meetings_dir();
            let m = tokio::task::spawn_blocking(move || storage::load_meeting(&dir, meeting_id))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            m.source_text()
        }
    } else {
        let Some(handle) = state.current() else {
            return Err("no meeting".into());
        };
        let s = handle.meeting.read().source_text();
        s
    };

    if transcript.trim().is_empty() {
        return Ok(String::new());
    }
    let (text, _usage) = claude
        .translate_full(&transcript)
        .await
        .map_err(|e| e.to_string())?;
    Ok(text)
}

// --- Transcript downloads -----------------------------------------------

/// Load the requested meeting (live if id matches the current meeting,
/// otherwise from disk). Returns the full Meeting struct so callers can
/// build either the raw or the formatted view.
async fn load_meeting_for_export(
    id: Option<Uuid>,
    state: &State<'_, Arc<AppState>>,
) -> Result<Meeting, String> {
    if let Some(handle) = state.current() {
        let live_id = handle.meeting.read().id;
        if id.map(|i| i == live_id).unwrap_or(true) {
            return Ok(handle.meeting.read().clone());
        }
    }
    let Some(meeting_id) = id else { return Err("no meeting".into()) };
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || storage::load_meeting(&dir, meeting_id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Replace anything that wouldn't be a friendly filename character with
/// underscores. Meeting titles can contain slashes, colons, etc. that
/// either break on disk or wrap awkwardly in Finder.
fn safe_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\n' | '\r' | '\t' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

/// %USERPROFILE%\Downloads is the universal "I'm going to grab this later"
/// spot on Windows. Write the file there, return the absolute path so the UI
/// can show the user exactly where it went.
fn write_to_downloads(
    title: &str,
    suffix: &str,
    content: &str,
) -> Result<std::path::PathBuf, String> {
    let home = std::env::var("USERPROFILE").map_err(|_| "USERPROFILE unset".to_string())?;
    let dir = std::path::PathBuf::from(home).join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create downloads dir: {e}"))?;
    let stem = safe_filename(title);
    let stem = if stem.is_empty() { "meeting".to_string() } else { stem };
    let path = dir.join(format!("{stem}-{suffix}.txt"));
    std::fs::write(&path, content).map_err(|e| format!("write {path:?}: {e}"))?;
    Ok(path)
}

/// Download the raw transcript with [HH:MM:SS] timestamps and (when
/// the meeting had more than one speaker) speaker labels. Pure text,
/// no LLM call, instant. Returns the absolute path of the written file.
#[tauri::command]
pub async fn export_raw_transcript_file(
    id: Option<Uuid>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let meeting = load_meeting_for_export(id, &state).await?;
    let text = meeting.formatted_transcript();
    if text.trim().is_empty() {
        return Err("transcript is empty".into());
    }
    let path = write_to_downloads(&meeting.title, "raw", &text)?;
    Ok(path.to_string_lossy().to_string())
}

/// Download a cleaned + translated transcript. Sends the formatted
/// transcript through the configured LLM with a prompt that asks it to
/// (1) clean up obvious transcription errors and (2) translate to the
/// target language, all while preserving the [HH:MM:SS] + speaker
/// structure. Returns the absolute path of the written file.
#[tauri::command]
pub async fn export_cleaned_translated_transcript_file(
    id: Option<Uuid>,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let an_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;
    let claude = LlmClient::from_settings(an_key, settings::read_target_language());
    let target = settings::read_target_language();

    let meeting = load_meeting_for_export(id, &state).await?;
    let formatted = meeting.formatted_transcript();
    if formatted.trim().is_empty() {
        return Err("transcript is empty".into());
    }

    // Split off the leading "# …" header lines (title / started / ended).
    // They're metadata, not speech — keep them verbatim, don't translate.
    let mut header_lines: Vec<&str> = Vec::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_header = true;
    for line in formatted.lines() {
        if in_header && (line.starts_with('#') || line.trim().is_empty()) {
            header_lines.push(line);
        } else {
            in_header = false;
            body_lines.push(line);
        }
    }

    // Chunk the body so we never blow the per-response token cap on long
    // meetings (a 1000+ segment news broadcast would truncate badly in a
    // single call). ~120 lines per chunk keeps each request comfortably
    // under the 8000-token output limit while preserving enough local
    // context for good cleanup.
    const CHUNK_LINES: usize = 120;
    let mut cleaned_parts: Vec<String> = Vec::new();
    for chunk in body_lines.chunks(CHUNK_LINES) {
        let chunk_text = chunk.join("\n");
        if chunk_text.trim().is_empty() {
            continue;
        }
        let (text, _usage) = claude
            .clean_and_translate(&chunk_text)
            .await
            .map_err(|e| e.to_string())?;
        let text = text.trim_matches('\n').to_string();
        if !text.is_empty() {
            cleaned_parts.push(text);
        }
    }

    if cleaned_parts.is_empty() {
        return Err("LLM returned an empty translation".into());
    }

    // Reassemble: original header (verbatim) + cleaned body chunks.
    let header = header_lines.join("\n");
    let body = cleaned_parts.join("\n");
    let out = if header.trim().is_empty() {
        body
    } else {
        format!("{}\n{}\n", header.trim_end(), body)
    };

    let path = write_to_downloads(
        &meeting.title,
        &format!("cleaned-{}", target.to_lowercase().replace(' ', "-")),
        &out,
    )?;
    Ok(path.to_string_lossy().to_string())
}

/// Rename a historical meeting on disk. The active meeting (if any) is
/// renamed via `set_meeting_title` instead — that path also updates in-memory
/// state and emits an event.
#[tauri::command]
pub async fn rename_meeting(
    id: Uuid,
    title: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    // If the renamed meeting is the one currently in progress, route through
    // the live path so listeners get the update event.
    if let Some(handle) = state.current() {
        if handle.meeting.read().id == id {
            handle.meeting.write().title = title.clone();
            let snap = handle.meeting.read().clone();
            state.emit("meeting:update", snap);
            return Ok(());
        }
    }
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let mut m = storage::load_meeting(&dir, id)?;
        m.title = title;
        storage::save_meeting(&dir, &m)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Merge `source` into `target`: segments and chat are concatenated and
/// re-sorted by timestamp, notes are appended, tags are unioned, speaker
/// names are merged (target wins on conflict), and cost fields are summed.
/// The source meeting is deleted after a successful save. Neither side may
/// be the currently-running meeting — stop it first.
#[tauri::command]
pub async fn merge_meetings(
    source: Uuid,
    target: Uuid,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    if source == target {
        return Err("cannot merge a meeting into itself".into());
    }
    if let Some(handle) = state.current() {
        let live = handle.meeting.read().id;
        if live == source || live == target {
            return Err("stop the active meeting before merging".into());
        }
    }
    let dir = state.meetings_dir();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let src = storage::load_meeting(&dir, source)?;
        let mut tgt = storage::load_meeting(&dir, target)?;

        // started_at: earliest of the two; ended_at: latest of the two.
        if src.started_at < tgt.started_at {
            tgt.started_at = src.started_at;
        }
        tgt.ended_at = match (tgt.ended_at, src.ended_at) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        // Segments: append + sort by started_at so combined transcript reads
        // chronologically even if the two recordings overlapped in time.
        tgt.segments.extend(src.segments.into_iter());
        tgt.segments.sort_by(|a, b| a.started_at.cmp(&b.started_at));

        // Chat: append + sort by timestamp.
        tgt.chat.extend(src.chat.into_iter());
        tgt.chat.sort_by(|a, b| a.at.cmp(&b.at));

        // Notes: concatenate with a separator if both are non-empty.
        if !src.notes.trim().is_empty() {
            if tgt.notes.trim().is_empty() {
                tgt.notes = src.notes;
            } else {
                tgt.notes.push_str("\n\n---\n\n");
                tgt.notes.push_str(&src.notes);
            }
        }

        // Tags: union, preserving target's order, then appending new ones.
        for t in src.tags {
            if !tgt.tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                tgt.tags.push(t);
            }
        }

        // Speaker names: target wins on conflict.
        for (k, v) in src.speaker_names {
            tgt.speaker_names.entry(k).or_insert(v);
        }

        // Cost: sum.
        tgt.cost.deepgram_audio_secs += src.cost.deepgram_audio_secs;
        tgt.cost.anthropic_input_tokens += src.cost.anthropic_input_tokens;
        tgt.cost.anthropic_output_tokens += src.cost.anthropic_output_tokens;
        tgt.cost.anthropic_cache_read_tokens += src.cost.anthropic_cache_read_tokens;

        // Summary is now stale; clear so the user knows to regenerate.
        tgt.summary = None;
        tgt.summary_updated_at = None;

        storage::save_meeting(&dir, &tgt)?;
        storage::delete_meeting(&dir, source)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn regenerate_summary(
    id: Option<Uuid>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let an_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;
    let claude = LlmClient::from_settings(an_key, settings::read_target_language());
    let app_state = state.inner().clone();

    // Decide which meeting we're summarizing: the live one if `id` matches
    // (or is None and a meeting is running), otherwise a historical meeting
    // loaded from disk.
    let live_handle = state.current().filter(|h| {
        match id {
            None => true,
            Some(want) => h.meeting.read().id == want,
        }
    });

    if let Some(handle) = live_handle {
        let transcript = handle.meeting.read().source_text();
        if transcript.trim().is_empty() {
            return Ok(());
        }
        let meeting = handle.meeting.clone();
        tokio::spawn(async move {
            match claude.summarize(&transcript).await {
                Ok((s, usage)) => {
                    let now = Utc::now();
                    {
                        let mut m = meeting.write();
                        m.summary = Some(s.clone());
                        m.summary_updated_at = Some(now);
                        m.cost.anthropic_input_tokens +=
                            usage.input_tokens + usage.cache_creation_input_tokens;
                        m.cost.anthropic_output_tokens += usage.output_tokens;
                        m.cost.anthropic_cache_read_tokens += usage.cache_read_input_tokens;
                    }
                    emit_cost(&app_state, &meeting);
                    app_state.emit(
                        "summary:update",
                        json!({ "summary": s, "updated_at": now }),
                    );
                }
                Err(err) => {
                    app_state.emit(
                        "error",
                        json!({ "message": format!("summary failed: {err}") }),
                    );
                }
            }
        });
        return Ok(());
    }

    // Historical meeting path: load → summarize → save → emit update so the
    // pane refreshes.
    let Some(meeting_id) = id else {
        return Err("no meeting".into());
    };
    let dir = state.meetings_dir();
    tokio::spawn(async move {
        let load_dir = dir.clone();
        let m = match tokio::task::spawn_blocking(move || storage::load_meeting(&load_dir, meeting_id))
            .await
        {
            Ok(Ok(m)) => m,
            other => {
                app_state.emit(
                    "error",
                    json!({ "message": format!("load failed: {other:?}") }),
                );
                return;
            }
        };
        let transcript = m.source_text();
        if transcript.trim().is_empty() {
            return;
        }
        match claude.summarize(&transcript).await {
            Ok((s, usage)) => {
                let now = Utc::now();
                let mut updated = m.clone();
                updated.summary = Some(s.clone());
                updated.summary_updated_at = Some(now);
                updated.cost.anthropic_input_tokens +=
                    usage.input_tokens + usage.cache_creation_input_tokens;
                updated.cost.anthropic_output_tokens += usage.output_tokens;
                updated.cost.anthropic_cache_read_tokens += usage.cache_read_input_tokens;
                let save_dir = dir.clone();
                let to_save = updated.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    storage::save_meeting(&save_dir, &to_save)
                })
                .await;
                app_state.emit(
                    "summary:update",
                    json!({ "summary": s, "updated_at": now }),
                );
                // Rebroadcast the meeting so the UI refreshes the pane.
                app_state.emit("meeting:update", updated);
            }
            Err(err) => {
                app_state.emit(
                    "error",
                    json!({ "message": format!("summary failed: {err}") }),
                );
            }
        }
    });
    Ok(())
}

// ------- chat -------

#[tauri::command]
pub async fn ask_question(
    question: String,
    meeting_id: Option<Uuid>,
    state: State<'_, Arc<AppState>>,
) -> Result<AskHandle, String> {
    if question.trim().is_empty() {
        return Err("empty question".into());
    }
    let an_key = settings::require_llm_credentials().map_err(|e| e.to_string())?;

    // Pick the meeting: an explicitly-supplied id wins (so the user can ask
    // questions of a saved meeting while a different one is being recorded),
    // otherwise fall back to the live meeting.
    let meeting_arc: Arc<RwLock<Meeting>> = if let Some(id) = meeting_id {
        if let Some(handle) = state.current().filter(|h| h.meeting.read().id == id) {
            handle.meeting.clone()
        } else {
            let dir = state.meetings_dir();
            let m = tokio::task::spawn_blocking(move || storage::load_meeting(&dir, id))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Arc::new(RwLock::new(m))
        }
    } else if let Some(handle) = state.current() {
        handle.meeting.clone()
    } else {
        return Err("no current meeting and no meeting_id given".into());
    };

    let stream_id = Uuid::new_v4();
    let app_state = state.inner().clone();
    let claude = LlmClient::from_settings(an_key, settings::read_target_language());

    // Snapshot transcript & history for the request. Source-language only —
    // Claude reads Dutch fine, and feeding the choppy per-chunk translations
    // produces worse answers than the original.
    let (transcript, history): (String, Vec<(String, String)>) = {
        let m = meeting_arc.read();
        (
            m.source_text(),
            m.chat
                .iter()
                .map(|c| (c.role.clone(), c.content.clone()))
                .collect(),
        )
    };

    // Persist the user message immediately.
    {
        let mut m = meeting_arc.write();
        m.chat.push(ChatMessage {
            role: "user".into(),
            content: question.clone(),
            at: Utc::now(),
        });
    }
    app_state.emit(
        "chat:user",
        json!({ "stream_id": stream_id, "question": question }),
    );

    // When an agent backend (Claude Code / Hermes) is selected, "Ask the
    // meeting" becomes the Brain2 Agent: it answers using the live transcript
    // PLUS read access to the user's files (long-term memory). Non-streaming —
    // emit the full answer on chat:done. Direct backend keeps the streaming
    // transcript-only chat below.
    let backend = crate::agent::current_backend();
    if backend != crate::agent::AgentBackend::Direct {
        let q = question.clone();
        let agent_transcript = transcript.clone();
        let agent_key = settings::require_anthropic().unwrap_or_default();
        let meeting_for_save = meeting_arc.clone();
        let dir_for_save = state.meetings_dir();
        let app_for_task = app_state.clone();
        tokio::spawn(async move {
            let root = std::path::PathBuf::from(
                std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into()),
            );
            match crate::agent::run_chat(backend, &q, &agent_transcript, &root, &agent_key).await {
                Ok(answer) if !answer.trim().is_empty() => {
                    let answer = answer.trim().to_string();
                    {
                        let mut m = meeting_for_save.write();
                        m.chat.push(ChatMessage {
                            role: "assistant".into(),
                            content: answer.clone(),
                            at: Utc::now(),
                        });
                    }
                    let snap = meeting_for_save.read().clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        storage::save_meeting(&dir_for_save, &snap)
                    })
                    .await;
                    app_for_task
                        .emit("chat:done", json!({ "stream_id": stream_id, "answer": answer }));
                }
                Ok(_) => app_for_task.emit(
                    "chat:done",
                    json!({ "stream_id": stream_id, "answer": "(no answer)" }),
                ),
                Err(e) => app_for_task.emit(
                    "chat:error",
                    json!({ "stream_id": stream_id, "error": format!("agent chat: {e}") }),
                ),
            }
        });
        return Ok(AskHandle { stream_id });
    }

    let q = question.clone();
    let meeting_for_save = meeting_arc.clone();
    let dir_for_save = state.meetings_dir();
    let app_for_task = app_state.clone();
    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel::<ChatStreamEvent>(32);

        let claude_task = tokio::spawn(async move {
            claude.chat_stream(&transcript, &history, &q, tx).await
        });

        let mut full = String::new();
        while let Some(evt) = rx.recv().await {
            match evt {
                ChatStreamEvent::Delta(d) => {
                    full.push_str(&d);
                    app_for_task
                        .emit("chat:delta", json!({ "stream_id": stream_id, "delta": d }));
                }
                ChatStreamEvent::Done(text) => {
                    full = text;
                }
                ChatStreamEvent::Error(err) => {
                    app_for_task.emit(
                        "chat:error",
                        json!({ "stream_id": stream_id, "error": err }),
                    );
                }
            }
        }
        let _ = claude_task.await;

        if !full.is_empty() {
            {
                let mut m = meeting_for_save.write();
                m.chat.push(ChatMessage {
                    role: "assistant".into(),
                    content: full.clone(),
                    at: Utc::now(),
                });
            }
            // best-effort save
            let snap = meeting_for_save.read().clone();
            let _ = tokio::task::spawn_blocking(move || storage::save_meeting(&dir_for_save, &snap))
                .await;
            app_for_task.emit(
                "chat:done",
                json!({ "stream_id": stream_id, "answer": full }),
            );
        }
    });

    Ok(AskHandle { stream_id })
}

// ------- meeting orchestrator -------

/// Spawn the local Whisper STT engine on the meeting's audio broadcast. It
/// emits the same `DeepgramEvent`s as the Deepgram path — a drop-in STT
/// producer. The model is ensured/downloaded inside the task so meeting start
/// isn't blocked.
#[cfg(feature = "local-stt")]
fn spawn_local_whisper(
    state: Arc<AppState>,
    audio_bcast: tokio::sync::broadcast::Sender<bytes::Bytes>,
    cancel: CancellationToken,
    dg_tx: mpsc::Sender<DeepgramEvent>,
) {
    let data_dir = state.data_dir.clone();
    let model_name = settings::read_whisper_model();
    let lang = settings::read_source_language();
    tokio::spawn(async move {
        let model_path = match crate::models::ensure_whisper_model(&data_dir, &model_name).await {
            Ok(p) => p,
            Err(e) => {
                state.emit(
                    "error",
                    json!({ "message": format!("Whisper model unavailable: {e}") }),
                );
                let _ = dg_tx
                    .send(DeepgramEvent::Status(deepgram::DgStatus::Disconnected))
                    .await;
                return;
            }
        };
        let _ = dg_tx
            .send(DeepgramEvent::Status(deepgram::DgStatus::Connected))
            .await;
        let mut bcast_rx = audio_bcast.subscribe();
        let (a_tx, a_rx) = mpsc::channel::<bytes::Bytes>(256);
        let adapter_cancel = cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = adapter_cancel.cancelled() => break,
                    r = bcast_rx.recv() => match r {
                        Ok(b) => { if a_tx.send(b).await.is_err() { break; } }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            }
        });
        let cfg = crate::local_stt::LocalSttConfig {
            model_path,
            gpu_device: 1,
            language: lang,
        };
        if let Err(e) = crate::local_stt::run(cfg, a_rx, dg_tx.clone(), cancel.clone()).await {
            let _ = dg_tx
                .send(DeepgramEvent::Error(format!("local STT: {e}")))
                .await;
        }
        let _ = dg_tx
            .send(DeepgramEvent::Status(deepgram::DgStatus::Disconnected))
            .await;
    });
}

async fn run_meeting(
    state: Arc<AppState>,
    handle: Arc<MeetingHandle>,
    brain: Arc<BrainEngine>,
    factory: Arc<FactoryConnector>,
    dg_key: String,
    an_key: String,
) -> Result<()> {
    let cancel = handle.cancel.clone();
    let meeting = handle.meeting.clone();

    // 1. Audio capture. System audio (WASAPI loopback) is always captured;
    //    the mic is optional (capture_mic setting). Both are mixed
    //    sample-aligned so overlapping speech is summed, not duplicated.
    //    Mic-off is the fix for speaker users, where the mic also picks up the
    //    system audio from the speakers and doubles the transcript.
    let include_mic = settings::read_capture_mic();
    let audio_rx =
        audio::start_capture(&state.app_handle, cancel.clone(), include_mic).await?;

    // 2. Broadcast audio so we can re-subscribe a fresh Deepgram session
    //    after a disconnect without losing the audio sidecar. When paused,
    //    we drop bytes here — sidecar keeps running, Deepgram sees nothing.
    let (audio_bcast, _) = tokio::sync::broadcast::channel::<bytes::Bytes>(256);
    {
        let bcast = audio_bcast.clone();
        let fwd_cancel = cancel.clone();
        let paused = handle.paused.clone();
        tokio::spawn(async move {
            let mut rx = audio_rx;
            loop {
                tokio::select! {
                    _ = fwd_cancel.cancelled() => break,
                    chunk = rx.recv() => match chunk {
                        Some(bytes) => {
                            if !paused.load(std::sync::atomic::Ordering::Relaxed) {
                                let _ = bcast.send(bytes);
                            }
                        }
                        None => break,
                    }
                }
            }
        });
    }

    // 3. Deepgram session inside a reconnect loop. Status events flow
    //    through dg_tx to the main meeting loop, which forwards them to
    //    the frontend as a `dg:status` event.
    let (dg_tx, mut dg_rx) = mpsc::channel::<DeepgramEvent>(256);
    let use_local = settings::read_stt_backend() == "local_whisper";

    // Local Whisper backend replaces the Deepgram session entirely (same event
    // stream). Feature-gated; without the feature, fall through with a notice.
    #[cfg(feature = "local-stt")]
    let local_spawned = if use_local {
        spawn_local_whisper(state.clone(), audio_bcast.clone(), cancel.clone(), dg_tx.clone());
        true
    } else {
        false
    };
    #[cfg(not(feature = "local-stt"))]
    let local_spawned = {
        if use_local {
            state.emit(
                "error",
                json!({ "message": "Local Whisper STT isn't in this build. Rebuild with --features local-stt, or set STT to Deepgram in Settings." }),
            );
        }
        false
    };

    if !local_spawned {
        let cfg_template = DeepgramConfig {
            api_key: dg_key,
            language: settings::read_source_language(),
            keyterms: settings::read_keywords(),
            ..Default::default()
        };
        let bcast = audio_bcast.clone();
        let cancel_dg = cancel.clone();
        let dg_tx_for_loop = dg_tx.clone();
        tokio::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                if cancel_dg.is_cancelled() { break; }
                let _ = dg_tx_for_loop
                    .send(DeepgramEvent::Status(deepgram::DgStatus::Connected))
                    .await;

                let mut bcast_rx = bcast.subscribe();
                let (audio_mpsc_tx, audio_mpsc_rx) = mpsc::channel::<bytes::Bytes>(128);
                let adapter_cancel = cancel_dg.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = adapter_cancel.cancelled() => break,
                            r = bcast_rx.recv() => match r {
                                Ok(bytes) => {
                                    if audio_mpsc_tx.send(bytes).await.is_err() { break; }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(_) => break,
                            }
                        }
                    }
                });

                let res = deepgram::run(
                    cfg_template.clone(),
                    audio_mpsc_rx,
                    dg_tx_for_loop.clone(),
                    cancel_dg.clone(),
                ).await;

                if cancel_dg.is_cancelled() { break; }
                tracing::warn!(?res, attempt, "deepgram session ended, retrying");

                attempt = attempt.saturating_add(1);
                let delay_ms: u64 = (500u64)
                    .saturating_mul(1u64 << attempt.min(5))
                    .min(30_000);
                let _ = dg_tx_for_loop.send(DeepgramEvent::Status(
                    deepgram::DgStatus::Reconnecting { attempt, retry_in_ms: delay_ms },
                )).await;

                let sleep_cancel = cancel_dg.clone();
                tokio::select! {
                    _ = sleep_cancel.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(delay_ms)) => {}
                }
            }
            let _ = dg_tx_for_loop
                .send(DeepgramEvent::Status(deepgram::DgStatus::Disconnected))
                .await;
        });
    }

    // Need a Clone of DeepgramConfig so the loop can clone per attempt.
    // (Clone derived below in the type; nothing to do here.)

    let claude = Arc::new(LlmClient::from_settings(an_key.clone(), settings::read_target_language()));

    let mut pending: Option<PendingSeg> = None;
    // Summaries are user-triggered only (regenerate_summary command). No
    // periodic auto-refresh — that just burned tokens and surprised users.
    let mut save_timer = tokio::time::interval(Duration::from_secs(15));
    save_timer.tick().await;

    // Central Brain ingestion. On a debounced timer we hand the latest
    // finalized transcript to the Brain engine — this is the single point
    // where everything the meeting captures (system audio, incl. Teams/Zoom
    // calls, plus the mic) feeds the 2nd brain. The busy flag stops runs from
    // overlapping so we never double-process or stall the transcription loop
    // while a Claude call is in flight.
    let mut brain_timer = tokio::time::interval(Duration::from_secs(30));
    brain_timer.tick().await;
    let brain_busy = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            evt = dg_rx.recv() => {
                let Some(evt) = evt else { break };
                handle_dg_event(evt, &mut pending, &state, &meeting, &claude).await;
            }
            _ = brain_timer.tick() => {
                let live_paused = handle.paused.load(Ordering::Relaxed);
                if !live_paused
                    && brain_busy
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok()
                {
                    let brain = brain.clone();
                    let meeting = meeting.clone();
                    let api = an_key.clone();
                    let busy = brain_busy.clone();
                    tokio::spawn(async move {
                        feed_brain(&brain, &meeting, &api).await;
                        busy.store(false, Ordering::Relaxed);
                    });
                }
            }
            _ = save_timer.tick() => {
                let snap = meeting.read().clone();
                let dir = state.meetings_dir();
                tokio::task::spawn_blocking(move || {
                    if let Err(err) = storage::save_meeting(&dir, &snap) {
                        tracing::warn!(?err, "save failed");
                    }
                });
            }
        }
    }

    // Flush + final save.
    {
        let mut m = meeting.write();
        m.ended_at = Some(Utc::now());
    }
    // Final Brain pass so any segments since the last timer tick are captured
    // before the meeting closes out.
    feed_brain(&brain, &meeting, &an_key).await;
    let snap = meeting.read().clone();
    let dir = state.meetings_dir();
    let _ = tokio::task::spawn_blocking(move || storage::save_meeting(&dir, &snap)).await;
    state.emit("meeting:stopped", meeting.read().clone());

    // Brain feeder: distill this finished meeting into a Knowledge note and
    // import it into gbrain, so the 2nd brain remembers what was discussed.
    // Best-effort + gated behind brain_feed_enabled inside distill_meeting.
    {
        let app = state.app_handle.clone();
        let snap = meeting.read().clone();
        tokio::spawn(async move {
            crate::feeder::distill_meeting(app, snap).await;
        });
    }

    // Best-effort telemetry to the AI Factory. Non-blocking: if the factory
    // isn't running on :3737 the send just fails quietly.
    {
        let factory = factory.clone();
        let brain_status = brain.status();
        let meeting_snap = meeting.read().clone();
        tokio::spawn(async move {
            let metrics = factory.build_metrics(
                1,
                meeting_snap.cost.deepgram_audio_secs,
                0.0,
                0.0,
                0,
                brain_status.action_items.len() as u64,
                brain_status.decisions.len() as u64,
            );
            if let Err(err) = factory.send_metrics(metrics).await {
                tracing::debug!(?err, "factory metrics send failed");
            }
        });
    }

    state.clear_current();
    Ok(())
}

/// Central Brain ingestion point. Snapshots the live transcript and hands the
/// new content to the Brain engine. Everything a meeting captures — system
/// audio (Teams, Zoom, browser calls) and the mic — lands here as finalized
/// segments, so this one call covers every source. `segment_count` is the
/// transcript line count, which the Brain engine uses to process only the
/// segments it hasn't seen yet.
async fn feed_brain(brain: &Arc<BrainEngine>, meeting: &Arc<RwLock<Meeting>>, api_key: &str) {
    let (id, title, transcript, seg_count) = {
        let m = meeting.read();
        let transcript = m.source_text();
        let seg_count = transcript.lines().count();
        (m.id, m.title.clone(), transcript, seg_count)
    };
    if transcript.trim().is_empty() {
        return;
    }
    if let Err(err) = brain
        .process_new_content(id, &title, &transcript, seg_count, api_key)
        .await
    {
        tracing::warn!(?err, "brain processing failed");
    }
}

/// Stability window — once a piece of text has appeared in interims for at
/// least this long, we anchor it. Anchored text never gets dropped from the
/// segment, even if a later interim from Deepgram doesn't include it.
///
/// Set to 0: anchor immediately on first sight. Anything Deepgram once
/// emitted as an interim is treated as "happened" and never disappears
/// from this chunk. False positives (Deepgram noise being kept) are
/// preferable to false negatives (losing real spoken content) for the
/// hyper-live use case.
const ANCHOR_STABILITY: std::time::Duration = std::time::Duration::from_millis(0);

/// Min overlap (in bytes) between the anchored suffix and a diverging new
/// interim's prefix to count as a continuation rather than a duplication.
/// Tuned just over a typical short-word length so "we" or "de" alone won't
/// glue unrelated sentences together.
const MERGE_MIN_OVERLAP: usize = 6;

/// One Deepgram chunk currently being transcribed.
///
/// As Interim events arrive we maintain `anchored` — the longest interim text
/// that has been observed unchanged for at least ANCHOR_STABILITY. Anchored
/// text is sticky: when Deepgram revises and produces a shorter or diverging
/// interim, we keep `anchored` and merge in whatever new tail Deepgram
/// produces (deduping any overlap with the anchored suffix). When `is_final`
/// fires, we apply the same merge to its text and commit the result as the
/// segment's dutch field.
struct PendingSeg {
    id: Uuid,
    started_at: chrono::DateTime<Utc>,
    /// Text that's been stable across multiple interims — never replaced.
    anchored: String,
    /// History of (observed_at, text) interims for recomputing anchored as
    /// time passes. Bounded.
    history: std::collections::VecDeque<(std::time::Instant, String)>,
    /// Diarization speaker id of the first interim that had one. Doesn't
    /// change mid-chunk — chunks rarely straddle speakers.
    speaker_id: Option<u32>,
}

impl PendingSeg {
    fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            started_at: Utc::now(),
            anchored: String::new(),
            history: std::collections::VecDeque::new(),
            speaker_id: None,
        }
    }

    /// Merge a new interim text into anchored state and return the display
    /// text the user should currently see.
    fn ingest_interim(&mut self, new_text: &str) -> String {
        let now = std::time::Instant::now();
        self.history.push_back((now, new_text.to_string()));
        while self.history.len() > 32 {
            self.history.pop_front();
        }
        // Anchored = longest interim text whose age >= ANCHOR_STABILITY.
        // We never let anchored shrink.
        let mut best: &str = self.anchored.as_str();
        for (t, s) in &self.history {
            if now.duration_since(*t) >= ANCHOR_STABILITY && s.len() > best.len() {
                best = s.as_str();
            }
        }
        if best.len() > self.anchored.len() {
            self.anchored = best.to_string();
        }
        merge_with_anchor(&self.anchored, new_text)
    }

    /// Produce the final segment text using Deepgram's authoritative is_final
    /// transcript merged with whatever we'd already anchored.
    fn finalize(&self, final_text: &str) -> String {
        merge_with_anchor(&self.anchored, final_text.trim())
    }

    fn to_segment(&self, dutch: String, is_final: bool) -> Segment {
        Segment {
            id: self.id,
            started_at: self.started_at,
            dutch,
            english: None,
            speaker: None,
            speaker_id: self.speaker_id,
            is_final,
        }
    }
}

/// Combine anchored text with a new interim/final by detecting overlap at
/// the seam, so we don't drop content but also don't duplicate it.
///
/// - If `anchored` is empty → just `new`.
/// - If `new` is a prefix-extension of `anchored` (Deepgram added more
///   words at the end) → use `new` (it's the longer cumulative text).
/// - If `anchored` is a prefix-extension of `new` (Deepgram revised down) →
///   keep `anchored` (sticky).
/// - If they share a non-trivial overlap (suffix of anchored == prefix of
///   new, ≥ MERGE_MIN_OVERLAP bytes) → splice them: anchored + new[overlap..].
/// - Otherwise → concat with a space (anchored + " " + new). This is the
///   case where Deepgram completely diverges; we preserve the anchored
///   words rather than throwing them away.
fn merge_with_anchor(anchored: &str, new: &str) -> String {
    let a = anchored.trim_end();
    let b = new.trim();
    if a.is_empty() { return b.to_string(); }
    if b.is_empty() { return a.to_string(); }
    if b == a || b.starts_with(a) { return b.to_string(); }
    if a.starts_with(b) { return a.to_string(); }

    // Find longest suffix of `a` that is a prefix of `b`.
    let max_check = a.len().min(b.len());
    let mut overlap = 0;
    let mut k = max_check;
    while k >= MERGE_MIN_OVERLAP {
        if a.is_char_boundary(a.len() - k) && b.is_char_boundary(k)
            && a[a.len() - k..].eq_ignore_ascii_case(&b[..k])
        {
            overlap = k;
            break;
        }
        k -= 1;
    }

    if overlap > 0 {
        format!("{}{}", a, &b[overlap..])
    } else {
        format!("{} {}", a, b)
    }
}

/// Treat anything starting with `en` (en, en-US, en-GB, …) as English.
fn is_english(lang: &Option<String>) -> bool {
    lang.as_deref()
        .map(|l| l.to_ascii_lowercase().starts_with("en"))
        .unwrap_or(false)
}

async fn handle_dg_event(
    evt: DeepgramEvent,
    pending: &mut Option<PendingSeg>,
    state: &Arc<AppState>,
    meeting: &Arc<RwLock<Meeting>>,
    claude: &Arc<LlmClient>,
) {
    match evt {
        DeepgramEvent::Stats { bytes_since_last } => {
            // 16-bit mono 16 kHz = 32000 bytes/sec → seconds = bytes/32000.
            let seconds = bytes_since_last as f64 / 32_000.0;
            {
                let mut m = meeting.write();
                m.cost.deepgram_audio_secs += seconds;
            }
            emit_cost(state, meeting);
            return;
        }
        DeepgramEvent::Status(s) => {
            let label = match s {
                deepgram::DgStatus::Connected => "connected",
                deepgram::DgStatus::Reconnecting { .. } => "reconnecting",
                deepgram::DgStatus::Disconnected => "disconnected",
            };
            let payload = match s {
                deepgram::DgStatus::Reconnecting { attempt, retry_in_ms } => {
                    json!({ "status": label, "attempt": attempt, "retry_in_ms": retry_in_ms })
                }
                _ => json!({ "status": label }),
            };
            state.emit("dg:status", payload);
            return;
        }
        DeepgramEvent::Interim { text, speaker, .. } => {
            let seg = pending.get_or_insert_with(PendingSeg::new);
            if speaker.is_some() && seg.speaker_id.is_none() {
                seg.speaker_id = speaker;
            }
            let display = seg.ingest_interim(&text);
            state.emit("segment:pending", seg.to_segment(display, false));
        }
        DeepgramEvent::Final { text, language, speaker, .. } => {
            // Each is_final=true closes a chunk → commit as its own segment
            // (live translation per chunk). The anchor merge ensures we
            // don't drop content Deepgram revised away mid-chunk.
            if text.trim().is_empty() {
                if let Some(p) = pending.as_mut() {
                    *p = PendingSeg::new();
                }
                return;
            }
            let mut p = pending.take().unwrap_or_else(PendingSeg::new);
            if speaker.is_some() && p.speaker_id.is_none() {
                p.speaker_id = speaker;
            }
            let dutch = p.finalize(&text);
            if dutch.trim().is_empty() { return; }
            let mut done = p.to_segment(dutch, true);

            let translate_on = settings::read_translate_enabled();
            if !translate_on || is_english(&language) {
                done.english = Some(done.dutch.clone());
                {
                    let mut m = meeting.write();
                    m.segments.push(done.clone());
                }
                state.emit("segment:upsert", done);
            } else {
                {
                    let mut m = meeting.write();
                    m.segments.push(done.clone());
                }
                state.emit("segment:upsert", done.clone());
                spawn_translate(state.clone(), meeting.clone(), claude.clone(), done);
            }
        }
        DeepgramEvent::UtteranceEnd => {
            // is_final commits chunks. Safety net for the case where the
            // stream ended with text still in flight.
            if let Some(p) = pending.take() {
                let dutch = p.anchored.trim().to_string();
                if !dutch.is_empty() {
                    let done = p.to_segment(dutch, true);
                    {
                        let mut m = meeting.write();
                        m.segments.push(done.clone());
                    }
                    state.emit("segment:upsert", done.clone());
                    spawn_translate(state.clone(), meeting.clone(), claude.clone(), done);
                }
            }
        }
        DeepgramEvent::Error(msg) => {
            state.emit("error", json!({ "message": msg }));
        }
        DeepgramEvent::Closed => {
            tracing::info!("deepgram session closed");
        }
    }
}

fn spawn_translate(
    state: Arc<AppState>,
    meeting: Arc<RwLock<Meeting>>,
    claude: Arc<LlmClient>,
    seg: Segment,
) {
    tokio::spawn(async move {
        match claude.translate(&seg.dutch).await {
            Ok((en, usage)) => {
                let en = en.trim().to_string();
                {
                    let mut m = meeting.write();
                    if let Some(s) = m.segments.iter_mut().find(|s| s.id == seg.id) {
                        s.english = Some(en.clone());
                    }
                    m.cost.anthropic_input_tokens += usage.input_tokens
                        + usage.cache_creation_input_tokens;
                    m.cost.anthropic_output_tokens += usage.output_tokens;
                    m.cost.anthropic_cache_read_tokens += usage.cache_read_input_tokens;
                }
                emit_cost(&state, &meeting);
                state.emit(
                    "segment:translated",
                    json!({ "id": seg.id, "english": en }),
                );
            }
            Err(err) => {
                tracing::warn!(?err, "translate failed");
                state.emit(
                    "segment:translated",
                    json!({
                        "id": seg.id,
                        "english": null,
                        "error": format!("{err}"),
                    }),
                );
            }
        }
    });
}

fn emit_cost(state: &Arc<AppState>, meeting: &Arc<RwLock<Meeting>>) {
    let cost = meeting.read().cost.clone();
    state.emit("cost:update", cost);
}

fn default_title() -> String {
    Utc::now().format("Meeting · %Y-%m-%d %H:%M").to_string()
}
