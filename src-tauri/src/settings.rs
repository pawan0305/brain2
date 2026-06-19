use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeys {
    pub deepgram: Option<String>,
    pub anthropic: Option<String>,
    /// Per-chunk Claude translation. When false, segments stay in the source
    /// language (no Claude calls per chunk). User can flip this from the
    /// top bar — for English meetings where translation is unnecessary.
    #[serde(default = "default_translate")]
    pub translate: bool,
    /// Capture the microphone (your own voice) alongside system audio. Turn
    /// off when listening on speakers — there the mic re-captures the system
    /// audio coming out of the speakers, which doubles the transcript. On
    /// headphones, leave it on. Default true.
    #[serde(default = "default_capture_mic")]
    pub capture_mic: bool,
    /// Subtitle overlay mode: "off" | "dual" (NL+EN) | "en" (EN only).
    #[serde(default = "default_overlay")]
    pub overlay_mode: String,
    /// Subtitle font size in px.
    #[serde(default = "default_overlay_size")]
    pub overlay_font_size: u32,
    /// When true the overlay is click-through (locked). When false the user
    /// can grab and drag/resize it.
    #[serde(default = "default_overlay_locked")]
    pub overlay_locked: bool,
    /// Custom vocabulary fed to Deepgram (`keyterm` parameter on Nova-3).
    /// One word/phrase per entry — colleague names, jargon, etc. Boosts
    /// transcription accuracy for those terms specifically.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Saved overlay window geometry (None = let Tauri center).
    #[serde(default)]
    pub overlay_x: Option<i32>,
    #[serde(default)]
    pub overlay_y: Option<i32>,
    #[serde(default)]
    pub overlay_w: Option<u32>,
    #[serde(default)]
    pub overlay_h: Option<u32>,
    /// Target language for Claude (translation, summary, chat). The source
    /// language is auto-detected by Deepgram. Stored as a human-readable
    /// language name ("English", "Spanish", "Japanese"…) so it drops
    /// straight into the prompts. Default "English".
    #[serde(default = "default_target_language")]
    pub target_language: String,
    /// Deepgram source-language code. "multi" (default) = auto-detect across
    /// Nova-3's multilingual set. A specific code like "nl" (Dutch), "nl-BE"
    /// (Flemish), "en", "de" locks Nova-3 to that single language, which is
    /// markedly more accurate than multi when you know what's being spoken.
    /// Applied when the Deepgram connection opens, i.e. on the next meeting.
    #[serde(default = "default_source_language")]
    pub source_language: String,
    /// Which LLM backend to use for translation / summary / chat.
    /// "anthropic" (default) uses api.anthropic.com with the configured
    /// anthropic key. "openai" uses any OpenAI-compatible endpoint —
    /// covers OpenAI itself, Ollama (localhost:11434/v1), LM Studio
    /// (localhost:1234/v1), llama.cpp's server, vLLM, OpenRouter, etc.
    #[serde(default = "default_llm_provider")]
    pub llm_provider: String,
    /// API key for the OpenAI-compatible endpoint. May be empty for a
    /// local model that doesn't enforce auth.
    #[serde(default)]
    pub openai_api_key: Option<String>,
    /// Base URL for the OpenAI-compatible endpoint. Empty = default
    /// "https://api.openai.com/v1". For Ollama set "http://localhost:11434/v1".
    #[serde(default)]
    pub openai_base_url: String,
    /// Model identifier for the OpenAI-compatible endpoint. Empty = a
    /// sensible default ("gpt-4o-mini"). For Ollama use e.g. "llama3.1:8b".
    #[serde(default)]
    pub openai_model: String,
    /// Which agent harness drives Brain2's reasoning (Brain engine + Forge).
    /// "direct" (default) = Anthropic Haiku directly; "claude_code" = the
    /// Claude Code CLI; "hermes" = the Hermes agent in WSL.
    #[serde(default = "default_agent_backend")]
    pub agent_backend: String,
    /// When agent_backend = "hermes": override Hermes's provider (e.g.
    /// "deepseek", "ollama"). Empty = Hermes's own default. This is the knob
    /// for pointing Brain2's brain at a local LLM later.
    #[serde(default)]
    pub hermes_provider: String,
    /// When agent_backend = "hermes": override Hermes's model. Empty = default.
    #[serde(default)]
    pub hermes_model: String,
    /// Model for the Claude Code backend (`claude --model`). The CLI's own
    /// default can be a preview model the account can't use headlessly, so we
    /// always pass an explicit one. Default "haiku".
    #[serde(default = "default_claude_model")]
    pub claude_model: String,
    /// Speech-to-text backend: "deepgram" (cloud, low-latency, default) or
    /// "local_whisper" (on-device whisper.cpp on the GPU — private + accurate,
    /// ~1-3s behind live). Only honored when the app is built with the
    /// `local-stt` feature.
    #[serde(default = "default_stt_backend")]
    pub stt_backend: String,
    /// Local Whisper model name for the local_whisper backend (manifest key in
    /// models.rs, e.g. "large-v3-q5_0").
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// Brain feeder — the continuous gbrain populator. When on, finished
    /// meetings and recent project work are distilled into the Knowledge folder
    /// and imported into gbrain. The user's master pause switch for the
    /// always-on 2nd-brain firehose.
    #[serde(default = "default_brain_feed_enabled")]
    pub brain_feed_enabled: bool,
    /// Repos the project-work feed watches (read-only). Windows paths (C:\…) or
    /// WSL paths (/home/…). Empty disables the project sweep.
    #[serde(default = "default_brain_feed_repos")]
    pub brain_feed_repos: Vec<String>,
    /// How often (minutes) the project-work sweep runs while the app is open.
    #[serde(default = "default_brain_feed_interval")]
    pub brain_feed_interval_mins: u64,
    /// RFC3339 timestamp of the last successful project sweep, so each run only
    /// looks at commits since then.
    #[serde(default)]
    pub brain_feed_since: Option<String>,
    /// The Knowledge folder gbrain imports from — where distilled notes land.
    #[serde(default = "default_knowledge_dir")]
    pub knowledge_dir: String,
}

impl Default for ApiKeys {
    fn default() -> Self {
        Self {
            deepgram: None,
            anthropic: None,
            translate: default_translate(),
            capture_mic: default_capture_mic(),
            overlay_mode: default_overlay(),
            overlay_font_size: default_overlay_size(),
            overlay_locked: default_overlay_locked(),
            keywords: vec![],
            overlay_x: None,
            overlay_y: None,
            overlay_w: None,
            overlay_h: None,
            target_language: default_target_language(),
            source_language: default_source_language(),
            llm_provider: default_llm_provider(),
            openai_api_key: None,
            openai_base_url: String::new(),
            openai_model: String::new(),
            agent_backend: default_agent_backend(),
            hermes_provider: String::new(),
            hermes_model: String::new(),
            claude_model: default_claude_model(),
            stt_backend: default_stt_backend(),
            whisper_model: default_whisper_model(),
            brain_feed_enabled: default_brain_feed_enabled(),
            brain_feed_repos: default_brain_feed_repos(),
            brain_feed_interval_mins: default_brain_feed_interval(),
            brain_feed_since: None,
            knowledge_dir: default_knowledge_dir(),
        }
    }
}

fn default_translate() -> bool { true }
fn default_capture_mic() -> bool { true }
fn default_overlay() -> String { "off".to_string() }
fn default_overlay_size() -> u32 { 24 }
fn default_overlay_locked() -> bool { true }
fn default_target_language() -> String { "English".to_string() }
fn default_source_language() -> String { "multi".to_string() }
fn default_llm_provider() -> String { "anthropic".to_string() }
fn default_agent_backend() -> String { "direct".to_string() }
fn default_claude_model() -> String { "haiku".to_string() }
fn default_stt_backend() -> String { "deepgram".to_string() }
fn default_whisper_model() -> String { "large-v3-q5_0".to_string() }
fn default_brain_feed_enabled() -> bool { true }
fn default_brain_feed_interval() -> u64 { 120 }
fn default_knowledge_dir() -> String {
    r"C:\Users\venkap\OneDrive - ICT Group\Knowledge".to_string()
}
/// The user's confirmed project-work watch-set (editable in Settings).
fn default_brain_feed_repos() -> Vec<String> {
    vec![
        r"C:\Users\venkap\projects\Brain2".to_string(),
        "/home/venkap/ai-factory".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsView {
    pub deepgram_set: bool,
    pub anthropic_set: bool,
    pub translate: bool,
    pub capture_mic: bool,
    pub overlay_mode: String,
    pub overlay_font_size: u32,
    pub overlay_locked: bool,
    pub keywords: Vec<String>,
    pub target_language: String,
    pub source_language: String,
    pub llm_provider: String,
    pub openai_set: bool,
    pub openai_base_url: String,
    pub openai_model: String,
    pub agent_backend: String,
    pub hermes_provider: String,
    pub hermes_model: String,
    pub claude_model: String,
    pub stt_backend: String,
    pub whisper_model: String,
    pub brain_feed_enabled: bool,
    pub brain_feed_repos: Vec<String>,
    pub brain_feed_interval_mins: u64,
    pub knowledge_dir: String,
}

/// %APPDATA%\com.brain2.app\keys.json — the same directory Tauri's
/// `app_data_dir()` resolves to on Windows, so keys.json sits alongside the
/// `meetings\` folder.
fn keys_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("com.brain2.app").join("keys.json")
}

/// Pre-rebrand keys location. Settings used to live here under the old
/// "OneTrueDutchie" identifier; [`migrate_legacy_keys`] moves them once.
fn legacy_keys_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base)
        .join("com.onetruedutchie.app")
        .join("keys.json")
}

/// One-time migration: if keys don't yet exist at the current
/// `com.brain2.app` location but do at the legacy `com.onetruedutchie.app`
/// one, copy them over — so the rebrand doesn't silently wipe the user's API
/// keys + settings. Safe to call on every startup (no-op once migrated).
pub fn migrate_legacy_keys() {
    let new = keys_path();
    if new.exists() {
        return;
    }
    let old = legacy_keys_path();
    if old.exists() {
        if let Some(dir) = new.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::copy(&old, &new);
    }
}

pub fn read_keys() -> Result<ApiKeys> {
    let path = keys_path();
    if !path.exists() {
        return Ok(ApiKeys::default());
    }
    let data = fs::read_to_string(&path).context("read keys file")?;
    // Tolerate a leading UTF-8 BOM — Notepad and PowerShell's `-Encoding utf8`
    // add one, and serde_json would otherwise fail on the leading bytes.
    let data = data.strip_prefix('\u{feff}').unwrap_or(&data);
    serde_json::from_str(data).context("parse keys file")
}

pub fn settings_view() -> Result<SettingsView> {
    let keys = read_keys()?;
    Ok(SettingsView {
        deepgram_set: keys.deepgram.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        anthropic_set: keys.anthropic.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        translate: keys.translate,
        capture_mic: keys.capture_mic,
        overlay_mode: keys.overlay_mode.clone(),
        overlay_font_size: keys.overlay_font_size,
        overlay_locked: keys.overlay_locked,
        keywords: keys.keywords.clone(),
        target_language: keys.target_language.clone(),
        source_language: keys.source_language.clone(),
        llm_provider: keys.llm_provider.clone(),
        openai_set: keys
            .openai_api_key
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        openai_base_url: keys.openai_base_url.clone(),
        openai_model: keys.openai_model.clone(),
        agent_backend: keys.agent_backend.clone(),
        hermes_provider: keys.hermes_provider.clone(),
        hermes_model: keys.hermes_model.clone(),
        claude_model: keys.claude_model.clone(),
        stt_backend: keys.stt_backend.clone(),
        whisper_model: keys.whisper_model.clone(),
        brain_feed_enabled: keys.brain_feed_enabled,
        brain_feed_repos: keys.brain_feed_repos.clone(),
        brain_feed_interval_mins: keys.brain_feed_interval_mins,
        knowledge_dir: keys.knowledge_dir.clone(),
    })
}

pub fn read_brain_feed_enabled() -> bool {
    read_keys().map(|k| k.brain_feed_enabled).unwrap_or(true)
}

pub fn set_brain_feed_enabled(on: bool) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.brain_feed_enabled = on;
    write_keys_back(&keys)
}

pub fn read_brain_feed_repos() -> Vec<String> {
    read_keys().map(|k| k.brain_feed_repos).unwrap_or_default()
}

pub fn set_brain_feed_repos(repos: Vec<String>) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.brain_feed_repos = repos
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    write_keys_back(&keys)
}

pub fn read_brain_feed_interval_mins() -> u64 {
    read_keys().map(|k| k.brain_feed_interval_mins).unwrap_or(120)
}

pub fn read_knowledge_dir() -> String {
    read_keys()
        .map(|k| k.knowledge_dir)
        .unwrap_or_else(|_| default_knowledge_dir())
}

pub fn read_brain_feed_since() -> Option<String> {
    read_keys().ok().and_then(|k| k.brain_feed_since)
}

pub fn set_brain_feed_since(ts: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.brain_feed_since = Some(ts.to_string());
    write_keys_back(&keys)
}

pub fn read_stt_backend() -> String {
    read_keys()
        .map(|k| k.stt_backend)
        .unwrap_or_else(|_| "deepgram".into())
}

pub fn set_stt_backend(backend: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.stt_backend = match backend.trim().to_ascii_lowercase().as_str() {
        "local_whisper" | "local" | "whisper" => "local_whisper".to_string(),
        _ => "deepgram".to_string(),
    };
    write_keys_back(&keys)
}

pub fn read_whisper_model() -> String {
    let m = read_keys().map(|k| k.whisper_model).unwrap_or_default();
    if m.trim().is_empty() {
        "large-v3-q5_0".to_string()
    } else {
        m
    }
}

pub fn set_whisper_model(model: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.whisper_model = model.trim().to_string();
    write_keys_back(&keys)
}

pub fn read_agent_backend() -> String {
    read_keys()
        .map(|k| k.agent_backend)
        .unwrap_or_else(|_| "direct".into())
}

pub fn set_agent_backend(backend: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.agent_backend = match backend.trim().to_ascii_lowercase().as_str() {
        "claude_code" | "claude-code" | "claudecode" | "claude" => "claude_code".to_string(),
        "hermes" => "hermes".to_string(),
        _ => "direct".to_string(),
    };
    write_keys_back(&keys)
}

pub fn read_hermes_config() -> (String, String) {
    let k = read_keys().unwrap_or_default();
    (k.hermes_provider, k.hermes_model)
}

pub fn set_hermes_config(provider: Option<&str>, model: Option<&str>) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    if let Some(p) = provider {
        keys.hermes_provider = p.trim().to_string();
    }
    if let Some(m) = model {
        keys.hermes_model = m.trim().to_string();
    }
    write_keys_back(&keys)
}

pub fn read_claude_model() -> String {
    let m = read_keys().map(|k| k.claude_model).unwrap_or_default();
    if m.trim().is_empty() {
        "haiku".to_string()
    } else {
        m
    }
}

pub fn set_claude_model(model: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.claude_model = model.trim().to_string();
    write_keys_back(&keys)
}

pub fn read_llm_provider() -> String {
    read_keys()
        .map(|k| k.llm_provider)
        .unwrap_or_else(|_| "anthropic".into())
}

pub fn read_openai_config() -> (String, String, String) {
    let k = read_keys().unwrap_or_default();
    (
        k.openai_api_key.unwrap_or_default(),
        if k.openai_base_url.is_empty() {
            "https://api.openai.com/v1".to_string()
        } else {
            k.openai_base_url
        },
        if k.openai_model.is_empty() {
            "gpt-4o-mini".to_string()
        } else {
            k.openai_model
        },
    )
}

pub fn set_llm_provider(provider: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    let normalized = match provider {
        "openai" | "OpenAI" | "openai-compatible" => "openai".to_string(),
        _ => "anthropic".to_string(),
    };
    keys.llm_provider = normalized;
    write_keys_back(&keys)
}

pub fn set_openai_config(
    api_key: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    if let Some(k) = api_key {
        keys.openai_api_key = if k.is_empty() { None } else { Some(k.to_string()) };
    }
    if let Some(u) = base_url {
        keys.openai_base_url = u.trim_end_matches('/').to_string();
    }
    if let Some(m) = model {
        keys.openai_model = m.to_string();
    }
    write_keys_back(&keys)
}

pub fn read_target_language() -> String {
    read_keys()
        .map(|k| k.target_language)
        .unwrap_or_else(|_| "English".into())
}

pub fn set_target_language(lang: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    let trimmed = lang.trim();
    keys.target_language = if trimmed.is_empty() {
        "English".into()
    } else {
        trimmed.to_string()
    };
    write_keys_back(&keys)
}

pub fn read_source_language() -> String {
    read_keys()
        .map(|k| k.source_language)
        .unwrap_or_else(|_| "multi".into())
}

pub fn set_source_language(code: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    let trimmed = code.trim();
    keys.source_language = if trimmed.is_empty() {
        "multi".into()
    } else {
        trimmed.to_string()
    };
    write_keys_back(&keys)
}

pub fn read_keywords() -> Vec<String> {
    read_keys().map(|k| k.keywords).unwrap_or_default()
}

pub fn set_keywords(words: Vec<String>) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.keywords = words
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    write_keys_back(&keys)
}

pub fn read_overlay_geometry() -> (Option<i32>, Option<i32>, Option<u32>, Option<u32>) {
    let k = read_keys().unwrap_or_default();
    (k.overlay_x, k.overlay_y, k.overlay_w, k.overlay_h)
}

pub fn set_overlay_geometry(x: i32, y: i32, w: u32, h: u32) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.overlay_x = Some(x);
    keys.overlay_y = Some(y);
    keys.overlay_w = Some(w);
    keys.overlay_h = Some(h);
    write_keys_back(&keys)
}

pub fn read_overlay_mode() -> String {
    read_keys().map(|k| k.overlay_mode).unwrap_or_else(|_| "off".into())
}

pub fn read_overlay_locked() -> bool {
    read_keys().map(|k| k.overlay_locked).unwrap_or(true)
}

fn write_keys_back(keys: &ApiKeys) -> Result<()> {
    let path = keys_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let data = serde_json::to_string_pretty(keys).context("serialize keys")?;
    fs::write(&path, &data).context("write keys file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn set_overlay_mode(mode: &str) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.overlay_mode = mode.to_string();
    write_keys_back(&keys)
}

pub fn set_overlay_font_size(size: u32) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    // Clamp to a sensible range so a typo can't make the overlay unusable.
    keys.overlay_font_size = size.clamp(12, 64);
    write_keys_back(&keys)
}

pub fn set_overlay_locked(locked: bool) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.overlay_locked = locked;
    write_keys_back(&keys)
}

pub fn read_capture_mic() -> bool {
    read_keys().map(|k| k.capture_mic).unwrap_or(true)
}

pub fn set_capture_mic(enabled: bool) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.capture_mic = enabled;
    write_keys_back(&keys)
}

pub fn read_translate_enabled() -> bool {
    read_keys().map(|k| k.translate).unwrap_or(true)
}

pub fn set_translate_enabled(enabled: bool) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    keys.translate = enabled;
    let path = keys_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let data = serde_json::to_string_pretty(&keys).context("serialize keys")?;
    fs::write(&path, &data).context("write keys file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn write_keys(deepgram: Option<&str>, anthropic: Option<&str>) -> Result<()> {
    let mut keys = read_keys().unwrap_or_default();
    if let Some(v) = deepgram {
        keys.deepgram = if v.is_empty() { None } else { Some(v.to_string()) };
    }
    if let Some(v) = anthropic {
        keys.anthropic = if v.is_empty() { None } else { Some(v.to_string()) };
    }
    let path = keys_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("create config dir")?;
    }
    let data = serde_json::to_string_pretty(&keys).context("serialize keys")?;
    fs::write(&path, &data).context("write keys file")?;
    // owner read/write only — equivalent to chmod 600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn require_deepgram() -> Result<String> {
    read_keys()?
        .deepgram
        .filter(|s| !s.is_empty())
        .context("Deepgram API key not set. Open Settings and add it.")
}

pub fn require_anthropic() -> Result<String> {
    read_keys()?
        .anthropic
        .filter(|s| !s.is_empty())
        .context("Anthropic API key not set. Open Settings and add it.")
}

/// Returns whatever credential the orchestrator needs to hand the LLM
/// client. For Anthropic mode that's the Anthropic key. For OpenAI mode
/// the key is read separately inside `LlmClient::from_settings`, so we
/// just return an empty string here — but we still validate that the
/// OpenAI side is at least minimally configured (model + base URL have
/// defaults, but a fully blank key on api.openai.com would 401, so we
/// error early when the base URL is api.openai.com and the key is
/// missing). Local endpoints may legitimately have no API key.
pub fn require_llm_credentials() -> Result<String> {
    match read_llm_provider().as_str() {
        "openai" => {
            let (key, base, _model) = read_openai_config();
            let is_openai_dot_com = base.starts_with("https://api.openai.com");
            if is_openai_dot_com && key.is_empty() {
                anyhow::bail!(
                    "OpenAI API key not set. Open Settings and add it, or point the base URL at a local model that doesn't need one.",
                );
            }
            Ok(String::new())
        }
        _ => require_anthropic(),
    }
}
