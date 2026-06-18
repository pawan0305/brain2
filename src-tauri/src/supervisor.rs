//! Stack supervisor — Brain2's "is everything that powers the 2nd brain up?"
//!
//! Brain2 is meant to be the one-click cockpit: you launch the app and the
//! whole local agentic stack it depends on comes alive. On startup this module
//! verifies — and, where it safely can, starts — those services, then reports
//! each one's health to the UI so the entire stack is visible in one window:
//!
//! - **WSL/Ubuntu** — the host for gbrain (and Hermes). Verified, not started
//!   (WSL spins up on first use).
//! - **Ollama** — local embeddings (gbrain) + local LLM. Auto-started if down.
//! - **gbrain** — the knowledge base ([[reference-gbrain]]). Health-probed via
//!   `gbrain health`.
//!
//! Claude Code's readiness is reported separately by the agent warm-up
//! (`agent:status`), so the UI can show the full row: WSL · Ollama · gbrain ·
//! Claude.

use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

const GBRAIN_PATH: &str = "export PATH=$HOME/.bun/bin:/usr/local/bin:/usr/bin:/bin;";
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
        check_wsl(&app).await;
        check_ollama(&app).await;
        check_gbrain(&app).await;
    });
}

async fn check_wsl(app: &AppHandle) {
    let out = tokio::process::Command::new("wsl")
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
    let _ = tokio::process::Command::new("ollama")
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

async fn check_gbrain(app: &AppHandle) {
    let script = format!("{GBRAIN_PATH} gbrain health 2>&1 | head -1");
    let out = tokio::process::Command::new("wsl")
        .arg("--")
        .arg("bash")
        .arg("-lc")
        .arg(&script)
        .output()
        .await;
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let line = stdout.lines().next().unwrap_or("").trim();
            if line.starts_with("Health score") {
                emit(app, "gbrain", "ok", line.to_string());
            } else if line.is_empty() {
                emit(app, "gbrain", "down", "gbrain not responding");
            } else {
                emit(app, "gbrain", "down", line.to_string());
            }
        }
        Err(_) => emit(app, "gbrain", "down", "could not run gbrain in WSL"),
    }
}
