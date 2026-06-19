//! The brain feeder — Brain2's continuous gbrain populator.
//!
//! Turns "what the user has been doing" into knowledge the 2nd brain can recall:
//!  - **Meetings** — when a meeting ends, distill its transcript + summary into
//!    a clean markdown note.
//!  - **Project work** — on an interval, summarize recent git activity in the
//!    watched repos (Brain2, AI Factory, …).
//!
//! Both are written into the user's Knowledge folder and then `gbrain import`ed
//! so they're searchable within seconds, not just at the next 30-min sync. The
//! distilling engine is the selected agent backend (Claude — see [[agent]]).
//! Everything is gated behind `brain_feed_enabled` + the repo watch-list, so the
//! always-on firehose stays under the user's control.
//!
//! ⚠️ Privacy: the user chose Claude Code as the distiller knowing this streams
//! meeting + project activity (often corporate) to Anthropic on a schedule.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::agent;
use crate::settings;
use crate::state::Meeting;

const GBRAIN_PATH: &str = "export PATH=$HOME/.bun/bin:/usr/local/bin:/usr/bin:/bin;";

fn knowledge_dir() -> PathBuf {
    PathBuf::from(settings::read_knowledge_dir())
}

fn emit_feed(app: &AppHandle, kind: &str, detail: impl Into<String>) {
    let _ = app.emit("feed:event", json!({ "kind": kind, "detail": detail.into() }));
}

// ── Meetings ─────────────────────────────────

/// Distill a finished meeting into a Knowledge note + import it. Best-effort.
pub async fn distill_meeting(app: AppHandle, meeting: Meeting) {
    if !settings::read_brain_feed_enabled() {
        return;
    }
    let transcript = meeting.source_text();
    if transcript.split_whitespace().count() < 40 {
        // Too short to be worth a knowledge note.
        return;
    }
    emit_feed(&app, "meeting:distilling", meeting.title.clone());
    match distill_meeting_inner(&meeting, &transcript).await {
        Ok(slug) => emit_feed(&app, "meeting:done", slug),
        Err(e) => {
            tracing::warn!(?e, "meeting distill failed");
            emit_feed(&app, "error", format!("meeting feed: {e}"));
        }
    }
}

async fn distill_meeting_inner(meeting: &Meeting, transcript: &str) -> Result<String> {
    let backend = agent::current_backend();
    let persona = agent::persona();
    let api_key = settings::require_anthropic().unwrap_or_default();
    let date = meeting.started_at.format("%Y-%m-%d").to_string();
    let system = "You are distilling a meeting into a concise knowledge-base note for the user's \
2nd brain. Output CLEAN MARKDOWN only (no preamble, no code fences): a 2-4 sentence summary, then a \
`## Decisions` section, an `## Action items` section (include owners if named), and a `## Key topics` \
section. Be faithful to the transcript — do not invent. Keep it tight.";
    let user = format!(
        "Meeting: {title}\nDate: {date}\n\nExisting rolling summary (may be partial):\n{summary}\n\n\
User notes:\n{notes}\n\nTranscript (source language):\n{transcript}",
        title = meeting.title,
        summary = meeting.summary.as_deref().unwrap_or("(none)"),
        notes = if meeting.notes.trim().is_empty() {
            "(none)"
        } else {
            meeting.notes.trim()
        },
    );
    let body = agent::run_text(backend, &persona, system, &user, &api_key)
        .await
        .context("distill meeting via agent")?;
    let slug = format!("{date}-{}", slugify(&meeting.title));
    let md = format!(
        "---\ntype: meeting\ntitle: \"{title}\"\ndate: {date}\nsource: brain2\n---\n\n# {title}\n\n{body}\n",
        title = meeting.title.replace('"', "'"),
        body = body.trim(),
    );
    write_note(&PathBuf::from("meetings").join(format!("{slug}.md")), &md)?;
    import_into_gbrain().await?;
    Ok(slug)
}

// ── Project work ─────────────────────────────

/// Spawn the periodic project-work sweep loop (runs while the app is open).
pub fn spawn_project_sweep(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Let startup settle before the first sweep.
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            if settings::read_brain_feed_enabled() {
                if let Err(e) = sweep_projects(&app).await {
                    tracing::warn!(?e, "project sweep failed");
                }
            }
            let mins = settings::read_brain_feed_interval_mins().max(15);
            tokio::time::sleep(Duration::from_secs(mins * 60)).await;
        }
    });
}

async fn sweep_projects(app: &AppHandle) -> Result<()> {
    let repos = settings::read_brain_feed_repos();
    if repos.is_empty() {
        return Ok(());
    }
    // Stamp the next watermark from BEFORE the sweep starts — a sweep can take
    // minutes (per-repo distillation), and commits landing during it must not
    // fall into a dead zone between the old and new watermark. And only advance
    // it if EVERY repo succeeded: a failed repo has to be retried next time, not
    // skipped forever.
    let started = Utc::now().to_rfc3339();
    let since = settings::read_brain_feed_since().unwrap_or_else(|| "24 hours ago".to_string());
    let mut all_ok = true;
    for repo in &repos {
        if let Err(e) = sweep_one_repo(app, repo, &since).await {
            all_ok = false;
            tracing::warn!(?e, repo = %repo, "repo sweep failed");
        }
    }
    if all_ok {
        let _ = settings::set_brain_feed_since(&started);
    }
    Ok(())
}

async fn sweep_one_repo(app: &AppHandle, repo: &str, since: &str) -> Result<bool> {
    let log = git_recent(repo, since).await?;
    if log.trim().is_empty() {
        return Ok(false); // nothing new since last sweep
    }
    let name = repo_name(repo);
    emit_feed(app, "project:distilling", name.clone());
    let backend = agent::current_backend();
    let persona = agent::persona();
    let api_key = settings::require_anthropic().unwrap_or_default();
    let system = "You summarize recent code activity for the user's 2nd brain. Given a git log + \
diffstat, write 2-5 plain markdown bullets describing WHAT changed and WHY (features, fixes, \
refactors, direction) — not a commit-by-commit dump. Bullets only, no preamble.";
    let user = format!("Project: {name}\nRecent git activity:\n{log}");
    let body = agent::run_text(backend, &persona, system, &user, &api_key)
        .await
        .context("distill project activity")?;
    let stamp = Utc::now().format("%Y-%m-%d %H:%M").to_string();
    let entry = format!("\n## {stamp} — recent work\n\n{}\n", body.trim());
    append_or_create_note(
        &PathBuf::from("projects").join(format!("{}.md", slugify(&name))),
        &name,
        &entry,
    )?;
    import_into_gbrain().await?;
    emit_feed(app, "project:done", name);
    Ok(true)
}

/// Recent commits + diffstat for a repo (Windows path runs git directly; WSL
/// path runs git inside WSL). Read-only.
async fn git_recent(repo: &str, since: &str) -> Result<String> {
    if repo.starts_with('/') {
        // No `2>/dev/null | head` pipe: keep git's exit status visible (wsl_out
        // turns a non-zero status into an Err), so a real git failure isn't
        // mistaken for "no new commits" (which would wrongly advance the
        // watermark). Truncate in Rust instead.
        let script = format!(
            "git -C '{repo}' log --since='{since}' --no-merges --date=short \
--pretty=format:'%h %ad %s' --shortstat"
        );
        let out = wsl_out(&script).await?;
        Ok(out.chars().take(8000).collect())
    } else {
        let out = tokio::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .arg("log")
            .arg(format!("--since={since}"))
            .arg("--no-merges")
            .arg("--date=short")
            .arg("--pretty=format:%h %ad %s")
            .arg("--shortstat")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .context("git log")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git log failed for {repo}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).chars().take(8000).collect())
    }
}

// ── gbrain + file helpers ────────────────────

/// Re-import the Knowledge folder into gbrain and embed new chunks — the same
/// incremental pair the 30-min cron runs, fired immediately so a fresh note is
/// searchable in seconds.
async fn import_into_gbrain() -> Result<()> {
    let wsl_kdir = to_wsl_path(&knowledge_dir());
    let script = format!(
        "{GBRAIN_PATH} gbrain import '{wsl_kdir}' --no-embed >/dev/null 2>&1; \
gbrain embed --stale >/dev/null 2>&1; echo ok"
    );
    let _ = wsl_out(&script).await?;
    Ok(())
}

async fn wsl_out(script: &str) -> Result<String> {
    let out = tokio::process::Command::new("wsl")
        .arg("--")
        .arg("bash")
        .arg("-lc")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("wsl command")?;
    if !out.status.success() {
        return Err(anyhow!(
            "wsl command failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn write_note(rel: &Path, contents: &str) -> Result<()> {
    let full = knowledge_dir().join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&full, contents).with_context(|| format!("write {}", full.display()))
}

fn append_or_create_note(rel: &Path, name: &str, entry: &str) -> Result<()> {
    let full = knowledge_dir().join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut content = std::fs::read_to_string(&full).unwrap_or_default();
    if content.trim().is_empty() {
        content = format!(
            "---\ntype: project\ntitle: \"{name}\"\nsource: brain2\n---\n\n# {name} — activity log\n"
        );
    }
    content.push_str(entry);
    std::fs::write(&full, content).with_context(|| format!("write {}", full.display()))
}

/// `C:\Users\…` → `/mnt/c/Users/…` for use inside WSL.
fn to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/' {
        let drive = (b[0] as char).to_ascii_lowercase();
        return format!("/mnt/{}{}", drive, &s[2..]);
    }
    s
}

fn repo_name(repo: &str) -> String {
    repo.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repo)
        .to_string()
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let s = out.trim_matches('-').to_string();
    if s.is_empty() {
        "untitled".to_string()
    } else {
        s
    }
}
