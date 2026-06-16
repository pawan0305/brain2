mod anthropic;
mod audio;
mod commands;
mod deepgram;
mod forge;
mod llm;
mod openai;
mod settings;
mod state;
mod storage;

use std::sync::{Arc, Mutex};

use tauri::Manager;
use tracing_subscriber::EnvFilter;

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
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("could not resolve app data dir");
            std::fs::create_dir_all(&data_dir).ok();

            let state = AppState::new(app_handle.clone(), data_dir.clone());
            app.manage(Arc::new(state));

            let forge = ForgeState::new(app_handle, data_dir);
            app.manage(Arc::new(forge));

            // Apply persisted overlay mode + lock. If the user had subtitles
            // on when they last quit, show the overlay window now and apply
            // the click-through state.
            let mode = settings::read_overlay_mode();
            let locked = settings::read_overlay_locked();
            let (gx, gy, gw, gh) = settings::read_overlay_geometry();
            if let Some(win) = app.get_webview_window("overlay") {
                // Restore saved position/size before showing.
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
            // Forge commands
            forge::forge_init,
            forge::forge_status,
            forge::forge_chat,
            forge::forge_diff,
            forge::forge_approve,
            forge::forge_reject,
            forge::forge_build,
            forge::forge_install,
            forge::forge_rollback,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
