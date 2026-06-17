//! Pluggable agent backend — Brain2's "brain".
//!
//! The reasoning behind Brain2 (the Brain engine's extraction / recall /
//! wrap-up, and the Forge self-improvement agent) can be driven by one of three
//! backends, selectable in Settings:
//!
//! - [`AgentBackend::Direct`]     — a direct Anthropic API call (Claude Haiku).
//!   The default; lowest latency, no external tooling.
//! - [`AgentBackend::ClaudeCode`] — the Claude Code CLI (`claude -p`), a real
//!   agent harness with native file/git/build tools.
//! - [`AgentBackend::Hermes`]     — the Hermes agent (`hermes -z`) in WSL. Its
//!   `--provider` / `-m` knobs are the path to running a *local* LLM later.
//!
//! Claude Code and Hermes share the SAME persona/instructions — the markdown in
//! `agent-prompts/BRAIN2.md` (embedded at compile time, overridable at runtime
//! from `%LOCALAPPDATA%\com.brain2.app\agent-prompts\BRAIN2.md`). The harness is
//! a swappable shell around one shared brain.
//!
//! Live transcript translation deliberately does NOT route through here: an
//! agent harness has a multi-second cold start per call — fine for "summarise
//! this meeting", fatal for per-sentence live subtitles. That stays on the fast
//! direct path in `llm.rs`.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::settings;

/// Default Brain2 persona, versioned in the repo and embedded at build time.
const DEFAULT_PERSONA: &str = include_str!("../../agent-prompts/BRAIN2.md");

const HAIKU_MODEL: &str = "claude-haiku-4-5-20251001";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentBackend {
    Direct,
    ClaudeCode,
    Hermes,
}

impl AgentBackend {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "claude_code" | "claude-code" | "claudecode" | "claude" => AgentBackend::ClaudeCode,
            "hermes" => AgentBackend::Hermes,
            _ => AgentBackend::Direct,
        }
    }
}

/// The backend currently selected in settings.
pub fn current_backend() -> AgentBackend {
    AgentBackend::parse(&settings::read_agent_backend())
}

fn agent_prompts_dir() -> Option<PathBuf> {
    let base = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        PathBuf::from(base)
            .join("com.brain2.app")
            .join("agent-prompts"),
    )
}

/// The shared persona — a runtime override file if present, else the embedded
/// default. Both harnesses (and the Direct backend) read the same text.
pub fn persona() -> String {
    if let Some(dir) = agent_prompts_dir() {
        if let Ok(s) = std::fs::read_to_string(dir.join("BRAIN2.md")) {
            if !s.trim().is_empty() {
                return s;
            }
        }
    }
    DEFAULT_PERSONA.to_string()
}

/// Write the default persona to the user-editable location on first run, so it
/// is discoverable and editable. Called once at startup.
pub fn seed_persona() {
    if let Some(dir) = agent_prompts_dir() {
        let path = dir.join("BRAIN2.md");
        if !path.exists() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, DEFAULT_PERSONA);
        }
    }
}

/// Run a one-shot reasoning task and return the agent's text output.
///
/// `system` is the task-specific instruction (e.g. "extract action items as a
/// JSON array"); the shared `persona` is always included. `api_key` is the
/// Anthropic key — used by Direct and Claude Code; Hermes uses its own config.
pub async fn run_text(
    backend: AgentBackend,
    persona: &str,
    system: &str,
    user: &str,
    api_key: &str,
) -> Result<String> {
    match backend {
        AgentBackend::Direct => direct_haiku(persona, system, user, api_key).await,
        AgentBackend::ClaudeCode => {
            let prompt = format!("{persona}\n\n{system}\n\n{user}");
            claude_code(&prompt, api_key, None).await
        }
        AgentBackend::Hermes => {
            let prompt = format!("{persona}\n\n{system}\n\n{user}");
            hermes(&prompt, None).await
        }
    }
}

/// Run an agentic coding task against a `workspace` (the Forge path). Claude
/// Code / Hermes edit files in the workspace directly (a clone of the Brain2
/// repo); the caller reviews the resulting git diff before committing/building.
/// Returns the agent's textual summary. `Direct` is not a coding agent —
/// callers keep their own patch flow for it.
pub async fn run_in_workspace(
    backend: AgentBackend,
    workspace: &Path,
    persona: &str,
    instruction: &str,
    api_key: &str,
) -> Result<String> {
    let prompt = format!(
        "{persona}\n\nYou are running inside a clone of the Brain2 repository at \
         your current working directory. Apply the following improvement by \
         editing files directly, keeping the change minimal and focused. Do NOT \
         commit — the user reviews the diff.\n\nREQUEST:\n{instruction}"
    );
    match backend {
        AgentBackend::ClaudeCode => claude_code(&prompt, api_key, Some(workspace)).await,
        AgentBackend::Hermes => hermes(&prompt, Some(workspace)).await,
        AgentBackend::Direct => Err(anyhow!(
            "the Direct backend has no workspace agent; use the built-in Forge patch flow"
        )),
    }
}

// ── Direct (Anthropic Haiku) ─────────────────

async fn direct_haiku(persona: &str, system: &str, user: &str, api_key: &str) -> Result<String> {
    let system_full = format!("{persona}\n\n{system}");
    let payload = serde_json::json!({
        "model": HAIKU_MODEL,
        "max_tokens": 2048,
        "system": [{"type": "text", "text": system_full}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": user}]}]
    });
    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&payload)
        .send()
        .await
        .context("anthropic request failed")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("anthropic error {status}: {body}"));
    }
    let body: serde_json::Value = resp.json().await.context("parse anthropic response")?;
    Ok(body["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default())
}

// ── Claude Code (`claude -p`) ────────────────

/// Spawn the Claude Code CLI in headless print mode, feeding the prompt on
/// stdin so no dynamic content has to survive Windows command-line quoting.
async fn claude_code(prompt: &str, api_key: &str, cwd: Option<&Path>) -> Result<String> {
    let mut cmd = claude_base_command();
    cmd.arg("-p").arg("--output-format").arg("text");
    if cwd.is_some() {
        // Editing the workspace: auto-accept edits — the user reviews the diff
        // afterwards via the Forge Approve/Reject gate.
        cmd.arg("--permission-mode").arg("acceptEdits");
    }
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if !api_key.is_empty() {
        cmd.env("ANTHROPIC_API_KEY", api_key);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .context("failed to launch `claude` — is Claude Code installed and on PATH?")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .context("write prompt to claude stdin")?;
        // Drop stdin to signal EOF so claude starts processing.
    }
    let out = child.wait_with_output().await.context("claude run failed")?;
    if !out.status.success() {
        return Err(anyhow!(
            "claude exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(windows)]
fn claude_base_command() -> Command {
    // The npm global `claude` is a .cmd/.ps1 shim (not a PE exe), so it must be
    // launched through cmd.exe. The prompt rides on stdin, so the only thing
    // cmd parses here are static, space-free flags.
    let mut c = Command::new("cmd");
    c.arg("/C").arg("claude");
    c
}

#[cfg(not(windows))]
fn claude_base_command() -> Command {
    Command::new("claude")
}

// ── Hermes (`hermes -z`) via WSL ─────────────

/// Spawn Hermes in WSL. The prompt is passed via the `BRAIN2_PROMPT` env var
/// (forwarded into WSL through `WSLENV`) so its content never goes through bash
/// quoting. `--provider` / `-m` from settings are the local-LLM switch.
async fn hermes(prompt: &str, cwd: Option<&Path>) -> Result<String> {
    let (provider, model) = settings::read_hermes_config();
    let mut script = String::from("hermes -z \"$BRAIN2_PROMPT\" --cli");
    if !provider.trim().is_empty() {
        script.push_str(&format!(" --provider {}", provider.trim()));
    }
    if !model.trim().is_empty() {
        script.push_str(&format!(" -m {}", model.trim()));
    }
    if cwd.is_some() {
        // Unattended file edits in the workspace.
        script.push_str(" --yolo");
    }

    let full = if let Some(dir) = cwd {
        format!("cd {} && {}", shell_quote(&to_wsl_path(dir)), script)
    } else {
        script
    };

    let mut cmd = Command::new("wsl");
    cmd.arg("--").arg("bash").arg("-lc").arg(full);
    cmd.env("BRAIN2_PROMPT", prompt);
    cmd.env("WSLENV", "BRAIN2_PROMPT/u");
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let out = cmd
        .output()
        .await
        .context("failed to launch Hermes via WSL — is WSL installed and `hermes` on PATH inside it?")?;
    if !out.status.success() {
        return Err(anyhow!(
            "hermes exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Convert a Windows path (`C:\Users\…`) to its WSL mount (`/mnt/c/Users/…`).
fn to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && &s[1..3.min(s.len())] == ":/" {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        return format!("/mnt/{}{}", drive, &s[2..]);
    }
    s
}

/// Single-quote a string for safe use inside a bash command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
