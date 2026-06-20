mod agent;
mod anthropic;
mod audio;
mod brain;
mod commands;
mod deepgram;
mod factory;
mod feeder;
mod forge;
mod knowledge;
mod llm;
#[cfg(feature = "local-stt")]
mod local_stt;
mod models;
mod openai;
mod proc;
mod settings;
mod state;
mod storage;
mod supervisor;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use tracing_subscriber::EnvFilter;

use crate::brain::BrainEngine;
use crate::factory::FactoryConnector;
use crate::forge::ForgeState;
use crate::state::AppState;

/// Log to %LOCALAPPDATA%\com.brain2.app\logs\brain2.log so the
/// installed app has somewhere to write tracing output (the release build runs
/// with the `windows` subsystem and has no attached console).
fn open_log_file() -> Option<std::fs::File> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    let dir = std::path::PathBuf::from(base)
        .join("com.brain2.app")
        .join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("brain2.log"))
        .ok()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,brain2_lib=debug"));
    if let Some(file) = open_log_file() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .try_init();
    }
    tracing::info!("Brain2 starting");

    tauri::Builder::default()
        .setup(|app| {
            let app_handle = app.handle().clone();

            // Recover settings/keys from the pre-rebrand folder if needed, so
            // the OneTrueDutchie → Brain2 rename doesn't lose the user's keys.
            settings::migrate_legacy_keys();

            // Don't `.expect()` here — a panic in setup() aborts before the
            // managed state is registered, after which EVERY stateful command
            // (brain_status, current_meeting, …) fails with "state not managed
            // … before using this command". Degrade gracefully instead.
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            std::fs::create_dir_all(&data_dir).ok();

            // Seed the shared agent persona (agent-prompts/BRAIN2.md) to disk so
            // it's editable; both Claude Code and Hermes read the same file.
            crate::agent::seed_persona();

            let state = AppState::new(app_handle.clone(), data_dir.clone());
            app.manage(Arc::new(state));

            let forge = ForgeState::new(app_handle.clone(), data_dir.clone());
            app.manage(Arc::new(forge));

            let brain = BrainEngine::new(app_handle.clone(), data_dir.clone());
            app.manage(Arc::new(brain));

            let factory = FactoryConnector::new(app_handle.clone(), data_dir);
            app.manage(Arc::new(factory));

            // Apply persisted overlay mode + lock.
            let mode = settings::read_overlay_mode();
            let locked = settings::read_overlay_locked();
            let (gx, gy, gw, gh) = settings::read_overlay_geometry();
            if let Some(win) = app.get_webview_window("overlay") {
                if let (Some(w), Some(h)) = (gw, gh) {
                    let _ = win.set_size(tauri::PhysicalSize::new(w, h));
                }
                if let (Some(x), Some(y)) = (gx, gy) {
                    let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                }
                if mode != "off" {
                    let _ = win.show();
                    let _ = win.set_always_on_top(true);
                    #[cfg(target_os = "macos")]
                    {
                        let _ = win.set_visible_on_all_workspaces(true);
                    }
                }
                let _ = win.set_ignore_cursor_events(locked);
            }

            // Warm the agent backend in the background so the Brain2 Agent is
            // hot the moment the user opens "Ask the meeting" — no cold start
            // mid-meeting. No-op when the backend is Direct.
            crate::commands::spawn_warm_up(app_handle.clone());

            // One-click cockpit: verify the local stack the 2nd brain runs on —
            // Knowledge folder, and (if Hermes is the backend) WSL + Ollama.
            crate::supervisor::spawn_check(app_handle.clone());

            // Brain feeder: periodically distill recent project work into the
            // Knowledge folder (meetings are distilled on meeting-end). No-op while disabled.
            crate::feeder::spawn_project_sweep(app_handle.clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::set_api_keys,
            commands::set_translate_enabled,
            commands::set_capture_mic,
            commands::set_overlay_mode,
            commands::set_overlay_font_size,
            commands::set_overlay_locked,
            commands::set_vocab,
            commands::set_target_language,
            commands::set_source_language,
            commands::set_llm_provider,
            commands::set_openai_config,
            commands::save_overlay_geometry,
            commands::set_meeting_notes,
            commands::set_meeting_tags,
            commands::start_meeting,
            commands::stop_meeting,
            commands::set_paused,
            commands::is_paused,
            commands::current_meeting,
            commands::list_meetings,
            commands::load_meeting,
            commands::delete_meeting,
            commands::rename_meeting,
            commands::merge_meetings,
            commands::export_english_transcript,
            commands::export_raw_transcript_file,
            commands::export_cleaned_translated_transcript_file,
            commands::ask_question,
            commands::regenerate_summary,
            commands::set_meeting_title,
            // Forge
            forge::forge_init,
            forge::forge_status,
            forge::forge_chat,
            forge::forge_diff,
            forge::forge_approve,
            forge::forge_reject,
            forge::forge_build,
            forge::forge_install,
            forge::forge_rollback,
            // Brain
            brain::brain_status,
            brain::brain_toggle,
            brain::brain_mark_action_done,
            brain::brain_wrap_up,
            // Factory
            factory::factory_status,
            factory::factory_ping,
            factory::factory_send_metrics,
            factory::factory_report_error,
            factory::factory_add_idea,
            factory::factory_check_update,
            // Agent backend
            commands::set_agent_backend,
            commands::set_hermes_config,
            commands::set_claude_model,
            commands::warm_agent,
            // Brain feeder
            commands::set_brain_feed_enabled,
            commands::set_brain_feed_repos,
            commands::set_knowledge_dir,
            // Local STT
            commands::set_stt_backend,
            commands::set_whisper_model,
            commands::download_model,
            commands::list_local_models,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
