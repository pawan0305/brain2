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
//! Knowledge retrieval uses a lean agentic search (see [[knowledge]]) — a fast
//! keyword search of the Knowledge folder, no embedding server or database.
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
            claude_code(&prompt, api_key, None, false).await
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
        AgentBackend::ClaudeCode => claude_code(&prompt, api_key, Some(workspace), true).await,
        AgentBackend::Hermes => hermes(&prompt, Some(workspace)).await,
        AgentBackend::Direct => Err(anyhow!(
            "the Direct backend has no workspace agent; use the built-in Forge patch flow"
        )),
    }
}

/// Agentic "Ask the meeting" — answer using the live transcript PLUS the user's
/// long-term knowledge. For all backends: the Knowledge folder is searched
/// (fast keyword match) and the relevant pages are injected into the prompt.
/// `read_root` is the working directory for file access.
pub async fn run_chat(
    backend: AgentBackend,
    question: &str,
    transcript: &str,
    read_root: &Path,
    api_key: &str,
) -> Result<String> {
    let persona = persona();

    // Lean agentic search: keyword-match the Knowledge folder, inject results.
    // No embedding server, no database — just fast file search.
    let knowledge = crate::knowledge::retrieve(question).await.unwrap_or_default();

    let prompt = if knowledge.trim().is_empty() {
        format!(
            "{persona}\n\nYou are Brain2 answering a question for the user during or after a \
meeting — their 2nd brain. You have READ access to files under your working directory (Read/Grep/Glob) \
as a fallback. Answer concisely in plain text (no markdown). If you can't find the answer, say so \
rather than guessing. Don't read credential/secret files.\n\n\
=== CURRENT MEETING TRANSCRIPT ===\n{transcript}\n\n=== QUESTION ===\n{question}"
        )
    } else {
        format!(
            "{persona}\n\nYou are Brain2 — the user's 2nd brain — answering a question during \
or after a meeting. Below is the most relevant knowledge retrieved from their Knowledge folder \
(keyword search), followed by the current meeting transcript — answer from it. You ALSO have READ \
access to files under your working directory if you need to dig beyond the retrieved slice. Reply \
concisely in plain text (no markdown). If the answer genuinely isn't available, say so rather than \
guessing.\n\n\
=== LONG-TERM KNOWLEDGE (from Knowledge folder) ===\n{knowledge}\n\n\
=== CURRENT MEETING TRANSCRIPT ===\n{transcript}\n\n=== QUESTION ===\n{question}"
        )
    };

    match backend {
        AgentBackend::ClaudeCode => {
            claude_code(&prompt, api_key, Some(read_root), false).await
        }
        AgentBackend::Hermes => {
            hermes(&prompt, Some(read_root)).await
        }
        AgentBackend::Direct => Err(anyhow!(
            "the Direct backend has no agentic chat; the built-in chat handles it"
        )),
    }
}

/// Warm up the selected agent backend so the first real "Ask the meeting" query
/// is fast. `claude -p` is stateless per call, so this can't hold a session
/// open — but a throwaway probe primes the OS file cache, the Node module cache
/// and the auth-token check, shaving seconds off the first real query. Just as
/// usefully, it surfaces a broken setup (CLI missing / not authenticated) at
/// app launch instead of mid-meeting. The reply is discarded; only success or
/// failure matters. No-op for `Direct` (an HTTP call has nothing to warm).
pub async fn warm_up(read_root: &Path, api_key: &str) -> Result<()> {
    const PROBE: &str = "Reply with exactly the single word: ready";
    match current_backend() {
        AgentBackend::ClaudeCode => claude_code(PROBE, api_key, Some(read_root), false)
            .await
            .map(|_| ()),
        AgentBackend::Hermes => hermes(PROBE, Some(read_root)).await.map(|_| ()),
        AgentBackend::Direct => Ok(()),
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
async fn claude_code(
    prompt: &str,
    api_key: &str,
    cwd: Option<&Path>,
    allow_edits: bool,
) -> Result<String> {
    let mut cmd = claude_base_command();
    cmd.arg("-p").arg("--output-format").arg("text");
    // Always pass an explicit model — the CLI's own default may be a preview
    // model the account can't use in headless mode.
    let model = settings::read_claude_model();
    if !model.trim().is_empty() {
        cmd.arg("--model").arg(model.trim());
    }
    if allow_edits {
        // Forge: auto-accept edits — the user reviews the diff afterwards via
        // the Approve/Reject gate. (Chat runs read-only: allow_edits = false.)
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
    let mut c = crate::proc::command("cmd");
    c.arg("/C").arg("claude");
    c
}

#[cfg(not(windows))]
fn claude_base_command() -> Command {
    crate::proc::command("claude")
}

// ── Hermes (`hermes -z`) via WSL ─────────────

/// Spawn Hermes in WSL. The prompt is passed via the `BRAIN2_PROMPT` env var
/// (forwarded into WSL through `WSLENV`) so its content never goes through bash
/// quoting. `--provider` / `-m` from settings are the local-LLM switch.
async fn hermes(prompt: &str, cwd: Option<&Path>) -> Result<String> {
    let (provider, model) = settings::read_hermes_config();
    let mut script = String::from("hermes -z \"$BRAIN2_PROMPT\" --cli");
    if !provider.trim().is_empty() {
        script.push_str(&format!(" --provider {}", shell_quote(provider.trim())));
    }
    if !model.trim().is_empty() {
        script.push_str(&format!(" -m {}", shell_quote(model.trim())));
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

    let mut cmd = crate::proc::command("wsl");
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
    let b = s.as_bytes();
    // Byte comparison (not slicing) — avoids a panic if a non-drive path begins
    // with an ASCII letter followed by a multi-byte char.
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        let drive = (b[0] as char).to_ascii_lowercase();
        return format!("/mnt/{}{}", drive, &s[2..]);
    }
    s
}

/// Single-quote a string for safe use inside a bash command.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
