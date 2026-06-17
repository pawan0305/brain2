//! Brain2 ↔ AI Factory connector.
//!
//! Reports usage metrics, error patterns, and improvement ideas back to the
//! AI Factory. Checks for factory-built updates. The factory's Cerberus crew
//! handles heavy architecture changes that the Forge can't do alone.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

// ── Types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryMetrics {
    pub app_version: String,
    pub meeting_count: u64,
    pub total_audio_seconds: f64,
    pub total_deepgram_cost_est: f64,
    pub total_anthropic_cost_est: f64,
    pub forge_changes: u64,
    pub brain_actions_detected: u64,
    pub brain_decisions_detected: u64,
    pub errors: Vec<ErrorReport>,
    pub improvement_ideas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorReport {
    pub message: String,
    pub count: u64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryUpdate {
    pub version: String,
    pub url: String,
    pub notes: String,
    pub published_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryStatus {
    pub connected: bool,
    pub factory_url: String,
    pub last_sync: Option<String>,
    pub available_update: Option<FactoryUpdate>,
    pub metrics_sent: u64,
}

const FACTORY_URL: &str = "http://127.0.0.1:3737";

pub struct FactoryConnector {
    app_handle: AppHandle,
    data_dir: PathBuf,
    inner: RwLock<FactoryInner>,
}

struct FactoryInner {
    connected: bool,
    last_sync: Option<String>,
    available_update: Option<FactoryUpdate>,
    metrics_sent: u64,
    errors: Vec<ErrorReport>,
    improvement_ideas: Vec<String>,
}

impl FactoryConnector {
    pub fn new(app_handle: AppHandle, data_dir: PathBuf) -> Self {
        let factory_dir = data_dir.join("factory");
        let _ = fs::create_dir_all(&factory_dir);

        let mut connector = Self {
            app_handle,
            data_dir: factory_dir.clone(),
            inner: RwLock::new(FactoryInner {
                connected: false,
                last_sync: None,
                available_update: None,
                metrics_sent: 0,
                errors: vec![],
                improvement_ideas: vec![],
            }),
        };

        connector.load_state();
        connector
    }

    /// Test connectivity to the AI Factory server.
    pub async fn ping(&self) -> Result<bool> {
        let client = reqwest::Client::new();
        match client
            .get(format!("{FACTORY_URL}/api/status"))
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                self.inner.write().connected = true;
                self.emit_status();
                Ok(true)
            }
            _ => {
                self.inner.write().connected = false;
                self.emit_status();
                Ok(false)
            }
        }
    }

    /// Send metrics to the AI Factory.
    pub async fn send_metrics(&self, metrics: FactoryMetrics) -> Result<()> {
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{FACTORY_URL}/api/metrics/brain2"))
            .json(&metrics)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .context("failed to send metrics")?;

        if resp.status().is_success() {
            let mut inner = self.inner.write();
            inner.metrics_sent += 1;
            inner.last_sync = Some(chrono::Utc::now().to_rfc3339());
            inner.connected = true;
            self.save_state();
            self.emit_status();
        }

        Ok(())
    }

    /// Report an error to the factory for pattern analysis.
    pub fn report_error(&self, message: &str) {
        let mut inner = self.inner.write();
        let now = chrono::Utc::now().to_rfc3339();
        if let Some(existing) = inner.errors.iter_mut().find(|e| e.message == message) {
            existing.count += 1;
            existing.last_seen = now;
        } else {
            inner.errors.push(ErrorReport {
                message: message.to_string(),
                count: 1,
                first_seen: now.clone(),
                last_seen: now,
            });
        }
        self.save_state();
    }

    /// Record an improvement idea for the factory.
    pub fn add_idea(&self, idea: &str) {
        let mut inner = self.inner.write();
        inner.improvement_ideas.push(idea.to_string());
        // Keep only the most recent 50 (drop oldest from the front).
        let len = inner.improvement_ideas.len();
        if len > 50 {
            inner.improvement_ideas.drain(0..len - 50);
        }
        self.save_state();
    }

    /// Check for updates from the AI Factory.
    pub async fn check_update(&self, current_version: &str) -> Result<Option<FactoryUpdate>> {
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{FACTORY_URL}/api/releases/brain2/latest"))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .context("failed to check updates")?;

        if resp.status().is_success() {
            let release: serde_json::Value = resp.json().await?;
            let version = release["version"].as_str().unwrap_or("0.0.0");
            if version != current_version {
                let update = FactoryUpdate {
                    version: version.to_string(),
                    url: release["download_url"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    notes: release["notes"].as_str().unwrap_or("").to_string(),
                    published_at: release["published_at"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                };
                self.inner.write().available_update = Some(update.clone());
                self.emit_status();
                return Ok(Some(update));
            }
        }

        self.inner.write().available_update = None;
        self.emit_status();
        Ok(None)
    }

    /// Build aggregate metrics for reporting.
    pub fn build_metrics(
        &self,
        meeting_count: u64,
        total_audio_seconds: f64,
        total_dg_cost: f64,
        total_an_cost: f64,
        forge_changes: u64,
        brain_actions: u64,
        brain_decisions: u64,
    ) -> FactoryMetrics {
        let inner = self.inner.read();
        FactoryMetrics {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            meeting_count,
            total_audio_seconds,
            total_deepgram_cost_est: total_dg_cost,
            total_anthropic_cost_est: total_an_cost,
            forge_changes,
            brain_actions_detected: brain_actions,
            brain_decisions_detected: brain_decisions,
            errors: inner.errors.clone(),
            improvement_ideas: inner.improvement_ideas.clone(),
        }
    }

    pub fn status(&self) -> FactoryStatus {
        let inner = self.inner.read();
        FactoryStatus {
            connected: inner.connected,
            factory_url: FACTORY_URL.into(),
            last_sync: inner.last_sync.clone(),
            available_update: inner.available_update.clone(),
            metrics_sent: inner.metrics_sent,
        }
    }

    // ── Persistence ──────────────────────────

    fn save_state(&self) {
        let inner = self.inner.read();
        let state = serde_json::json!({
            "last_sync": inner.last_sync,
            "metrics_sent": inner.metrics_sent,
            "errors": inner.errors,
            "improvement_ideas": inner.improvement_ideas,
        });
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = fs::write(self.data_dir.join("factory_state.json"), json);
        }
    }

    fn load_state(&mut self) {
        let path = self.data_dir.join("factory_state.json");
        if let Ok(data) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(&data) {
                let mut inner = self.inner.write();
                inner.last_sync = state["last_sync"].as_str().map(|s| s.to_string());
                inner.metrics_sent = state["metrics_sent"].as_u64().unwrap_or(0);
                if let Some(errs) = state["errors"].as_array() {
                    if let Ok(parsed) =
                        serde_json::from_value(serde_json::Value::Array(errs.clone()))
                    {
                        inner.errors = parsed;
                    }
                }
                if let Some(ideas) = state["improvement_ideas"].as_array() {
                    inner.improvement_ideas = ideas
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                }
            }
        }
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app_handle.emit("factory:status", status);
    }
}

// ── Tauri commands ───────────────────────────

use tauri::State;

#[tauri::command]
pub async fn factory_status(
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<FactoryStatus, String> {
    Ok(factory.status())
}

#[tauri::command]
pub async fn factory_ping(
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<bool, String> {
    factory.ping().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn factory_send_metrics(
    meeting_count: u64,
    total_audio_seconds: f64,
    total_dg_cost: f64,
    total_an_cost: f64,
    forge_changes: u64,
    brain_actions: u64,
    brain_decisions: u64,
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<(), String> {
    let metrics = factory.build_metrics(
        meeting_count,
        total_audio_seconds,
        total_dg_cost,
        total_an_cost,
        forge_changes,
        brain_actions,
        brain_decisions,
    );
    factory.send_metrics(metrics).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn factory_report_error(
    message: String,
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<(), String> {
    factory.report_error(&message);
    Ok(())
}

#[tauri::command]
pub async fn factory_add_idea(
    idea: String,
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<(), String> {
    factory.add_idea(&idea);
    Ok(())
}

#[tauri::command]
pub async fn factory_check_update(
    current_version: String,
    factory: State<'_, Arc<FactoryConnector>>,
) -> Result<Option<FactoryUpdate>, String> {
    factory
        .check_update(&current_version)
        .await
        .map_err(|e| e.to_string())
}
