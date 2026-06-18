//! Local model management — download + locate the Whisper ggml models the
//! local STT backend uses. Models live under `<app_data>/models/whisper/` and
//! are fetched on demand (not bundled, to keep the installer small). Not gated
//! behind `local-stt` — the manifest/list/download commands work in any build;
//! only the engine that *uses* the models is feature-gated.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

pub struct WhisperModelSpec {
    pub name: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub approx_mb: u32,
}

/// ggml Whisper models from the official whisper.cpp HF repo. large-v3-q5_0 is
/// the default (best Dutch accuracy at a manageable size; ~3x realtime on the
/// Arc per the Phase-0 benchmark). Smaller ones are fallbacks for weaker HW.
pub const WHISPER_MODELS: &[WhisperModelSpec] = &[
    WhisperModelSpec {
        name: "large-v3-q5_0",
        file: "ggml-large-v3-q5_0.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",
        approx_mb: 1080,
    },
    WhisperModelSpec {
        name: "large-v3-turbo",
        file: "ggml-large-v3-turbo.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        approx_mb: 1560,
    },
    WhisperModelSpec {
        name: "small",
        file: "ggml-small.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        approx_mb: 466,
    },
    WhisperModelSpec {
        name: "base",
        file: "ggml-base.bin",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        approx_mb: 142,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub approx_mb: u32,
    pub downloaded: bool,
}

pub fn whisper_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models").join("whisper")
}

pub fn model_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    WHISPER_MODELS
        .iter()
        .find(|m| m.name == name)
        .map(|m| whisper_dir(data_dir).join(m.file))
}

pub fn is_downloaded(data_dir: &Path, name: &str) -> bool {
    model_path(data_dir, name)
        .map(|p| p.exists())
        .unwrap_or(false)
}

pub fn list_models(data_dir: &Path) -> Vec<ModelInfo> {
    WHISPER_MODELS
        .iter()
        .map(|m| ModelInfo {
            name: m.name.to_string(),
            approx_mb: m.approx_mb,
            downloaded: is_downloaded(data_dir, m.name),
        })
        .collect()
}

/// Ensure the named Whisper model is present locally, downloading it if needed.
/// Returns the path to the model file. Download is atomic (.part -> rename).
pub async fn ensure_whisper_model(data_dir: &Path, name: &str) -> Result<PathBuf> {
    let spec = WHISPER_MODELS
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| anyhow!("unknown whisper model '{name}'"))?;
    let dir = whisper_dir(data_dir);
    std::fs::create_dir_all(&dir).context("create models dir")?;
    let dest = dir.join(spec.file);
    if dest.exists() {
        return Ok(dest);
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(spec.url)
        .send()
        .await
        .context("request model download")?;
    if !resp.status().is_success() {
        return Err(anyhow!("model download failed: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.context("read model body")?;
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).context("write model file")?;
    std::fs::rename(&tmp, &dest).context("finalize model file")?;
    Ok(dest)
}
