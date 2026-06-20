//! Stack supervisor — Brain2's "is everything that powers the 2nd brain up?"
//!
//! Brain2 is meant to be the one-click cockpit: you launch the app and the
//! whole local agentic stack it depends on comes alive. On startup this module
//! verifies — and, where it safely can, starts — those services, then reports
//! each one's health to the UI so the entire stack is visible in one window:
//!
//! - **WSL/Ubuntu** — the host for Hermes (only relevant when Hermes is the
//!   selected backend). Verified, not started (WSL spins up on first use).
//! - **Ollama** — local LLM (optional, only for Hermes with a local model).
//!   Auto-started if down.
//! - **Knowledge** — the user's Knowledge folder (markdown files on disk).
//!   Health-probed by checking the folder exists and counting files.
//!
//! Claude Code's readiness is reported separately by the agent warm-up
//! (`agent:status`), so the UI can show the full row: Knowledge · Claude/Hermes.

use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

const OLLAMA_TAGS_URL: &str = "http://127.0.0.1:11434/api/tags";

#[derive(Serialize, Clone)]
struct Health {
    component: &'static str,
    /// "ok" | "starting" | "down"
    state: &'static str,
    detail: String,
}

fn emit(app: &AppHandle, component: &'static str, state: &'static str, detail: impl Into<String>) {
    let _ = app.emit(
        "stack:health",
        Health {
            component,
            state,
            detail: detail.into(),
        },
    );
}

/// Probe (and best-effort start) the whole stack in the background, emitting a
/// `stack:health` event per component as each check resolves.
pub fn spawn_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Knowledge folder is always checked — it's the core of the 2nd brain.
        check_knowledge(&app).await;

        // WSL + Ollama are only relevant when Hermes is the backend.
        let backend = crate::agent::current_backend();
        if backend == crate::agent::AgentBackend::Hermes {
            check_wsl(&app).await;
            check_ollama(&app).await;
        }
    });
}

async fn check_knowledge(app: &AppHandle) {
    let (ok, detail) = crate::knowledge::check_health();
    if ok {
        emit(app, "knowledge", "ok", detail);
    } else {
        emit(app, "knowledge", "down", detail);
    }
}

async fn check_wsl(app: &AppHandle) {
    let out = crate::proc::command("wsl")
        .arg("--")
        .arg("bash")
        .arg("-lc")
        .arg("echo ok")
        .output()
        .await;
    match out {
        Ok(o) if String::from_utf8_lossy(&o.stdout).contains("ok") => {
            emit(app, "wsl", "ok", "Ubuntu reachable")
        }
        _ => emit(app, "wsl", "down", "WSL not reachable"),
    }
}

async fn ollama_up() -> bool {
    reqwest::Client::new()
        .get(OLLAMA_TAGS_URL)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

async fn check_ollama(app: &AppHandle) {
    if ollama_up().await {
        emit(app, "ollama", "ok", "running on :11434");
        return;
    }
    // Down — try to start it (detached, fire-and-forget; Ollama is designed to
    // run as a long-lived server, so we deliberately don't track/kill it).
    emit(app, "ollama", "starting", "launching Ollama…");
    let _ = crate::proc::command("ollama")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(700)).await;
        if ollama_up().await {
            emit(app, "ollama", "ok", "running on :11434");
            return;
        }
    }
    emit(app, "ollama", "down", "could not start — open Ollama manually");
}
