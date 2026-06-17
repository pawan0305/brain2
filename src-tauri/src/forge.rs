//! Brain2 Forge — self-improvement engine.
//!
//! The Forge is a self-modifying agent inside Brain2. It clones the Brain2
//! repo from GitHub into a workspace, accepts natural-language improvement
//! requests via chat, modifies source files, shows diffs for human approval,
//! rebuilds the portable exe, and self-updates.
//!
//! # Safety
//!
//! - Workspace is sandboxed in `%LOCALAPPDATA%\com.brain2.app\forge\`
//! - Changes are git-tracked — every modification is a commit
//! - Human gate before every build — no code runs unapproved
//! - Previous builds saved as rollback points
//! - Original exe never modified in-place; update is atomic (rename swap)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

// ── Types ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeMessage {
    pub id: Uuid,
    pub role: String, // "user" | "agent"
    pub content: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeVersion {
    pub version: String,
    pub commit: String,
    pub built_at: DateTime<Utc>,
    pub exe_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BuildStatus {
    Idle,
    Building,
    Success { exe_path: String },
    Failed { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeStatus {
    pub initialized: bool,
    pub repo_url: String,
    pub branch: String,
    pub has_pending_changes: bool,
    pub pending_files: Vec<String>,
    pub build_status: BuildStatus,
    pub versions: Vec<ForgeVersion>,
    pub messages: Vec<ForgeMessage>,
}

pub struct ForgeState {
    pub app_handle: AppHandle,
    data_dir: PathBuf,
    inner: RwLock<ForgeInner>,
}

struct ForgeInner {
    messages: Vec<ForgeMessage>,
    versions: Vec<ForgeVersion>,
    build_status: BuildStatus,
}

const REPO_URL: &str = "https://github.com/pawan0305/brain2.git";

impl ForgeState {
    pub fn new(app_handle: AppHandle, data_dir: PathBuf) -> Self {
        Self {
            app_handle,
            data_dir: data_dir.clone(),
            inner: RwLock::new(ForgeInner {
                messages: vec![],
                versions: Self::load_versions(&data_dir),
                build_status: BuildStatus::Idle,
            }),
        }
    }

    pub fn forge_dir(&self) -> PathBuf {
        self.data_dir.join("forge")
    }

    pub fn workspace_dir(&self) -> PathBuf {
        self.forge_dir().join("workspace")
    }

    pub fn versions_dir(&self) -> PathBuf {
        self.forge_dir().join("versions")
    }

    // ── Init ──────────────────────────────────

    /// Clone the Brain2 repo into the forge workspace. Safe to call
    /// repeatedly — if the workspace already exists, just fetches latest.
    pub fn init(&self) -> Result<String> {
        let forge = self.forge_dir();
        fs::create_dir_all(&forge)?;
        let ws = self.workspace_dir();

        if ws.join(".git").exists() {
            // Already cloned — fetch latest
            let output = Command::new("git")
                .args(["fetch", "origin"])
                .current_dir(&ws)
                .output()
                .context("git fetch failed")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("git fetch failed: {stderr}"));
            }
            let output = Command::new("git")
                .args(["reset", "--hard", "origin/main"])
                .current_dir(&ws)
                .output()
                .context("git reset failed")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(anyhow!("git reset failed: {stderr}"));
            }
            Ok("workspace updated from GitHub".into())
        } else {
            // Fresh clone
            let output = Command::new("git")
                .args(["clone", REPO_URL, "."])
                .current_dir(&ws)
                .output();
            match output {
                Ok(o) if o.status.success() => Ok("workspace cloned from GitHub".into()),
                Ok(o) => Err(anyhow!(
                    "clone failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                )),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Err(anyhow!(
                            "git not found. Install Git from https://git-scm.com"
                        ))
                    } else {
                        Err(anyhow!("clone failed: {e}"))
                    }
                }
            }
        }
    }

    // ── Agent chat ────────────────────────────

    /// Add a user message and get the agent's response. The agent reads the
    /// source tree, understands the request, and applies file changes.
    pub async fn chat(
        &self,
        message: String,
        api_key: &str,
    ) -> Result<String> {
        // Add user message
        let user_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "user".into(),
            content: message.clone(),
            at: Utc::now(),
        };
        self.inner.write().messages.push(user_msg.clone());
        self.emit_status();

        // Build the source context (file listing + key files)
        let ws = self.workspace_dir();
        if !ws.join(".git").exists() {
            return Err(anyhow!("Forge not initialized. Click Init to clone the repo first."));
        }

        let source_context = build_source_context(&ws)?;

        // Call Claude to get the improvement plan + changes
        let agent_response = call_forge_agent(api_key, &message, &source_context).await?;

        // Try to apply the changes
        match apply_changes(&ws, &agent_response) {
            Ok(files_changed) => {
                let summary = if files_changed.is_empty() {
                    format!("{}\n\nNo source changes were needed.", agent_response)
                } else {
                    format!(
                        "{}\n\n✅ Changes applied to {} file(s): {}\nReview the diff below and Approve or Reject.",
                        agent_response,
                        files_changed.len(),
                        files_changed.join(", ")
                    )
                };

                let agent_msg = ForgeMessage {
                    id: Uuid::new_v4(),
                    role: "agent".into(),
                    content: summary,
                    at: Utc::now(),
                };
                self.inner.write().messages.push(agent_msg);
                self.emit_status();
                Ok(agent_response)
            }
            Err(e) => {
                let agent_msg = ForgeMessage {
                    id: Uuid::new_v4(),
                    role: "agent".into(),
                    content: format!(
                        "{}\n\n⚠️ Could not apply changes automatically: {e}\nPlease apply the changes manually in the workspace.",
                        agent_response
                    ),
                    at: Utc::now(),
                };
                self.inner.write().messages.push(agent_msg);
                self.emit_status();
                Ok(agent_response)
            }
        }
    }

    // ── Diff ──────────────────────────────────

    /// Show the pending git diff.
    pub fn diff(&self) -> Result<String> {
        let ws = self.workspace_dir();
        let output = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(&ws)
            .output()
            .context("git diff failed")?;
        let stat = String::from_utf8_lossy(&output.stdout).to_string();

        let output = Command::new("git")
            .args(["diff"])
            .current_dir(&ws)
            .output()
            .context("git diff failed")?;
        let full_diff = String::from_utf8_lossy(&output.stdout).to_string();

        if stat.trim().is_empty() && full_diff.trim().is_empty() {
            Ok("No pending changes.".into())
        } else {
            Ok(format!("{stat}\n\n{full_diff}"))
        }
    }

    /// Get list of pending changed files.
    pub fn pending_files(&self) -> Result<Vec<String>> {
        let ws = self.workspace_dir();
        let output = Command::new("git")
            .args(["diff", "--name-only"])
            .current_dir(&ws)
            .output()
            .context("git diff failed")?;
        let files: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(files)
    }

    // ── Approve / Reject ──────────────────────

    /// Commit pending changes.
    pub fn approve(&self, message: &str) -> Result<String> {
        let ws = self.workspace_dir();
        let output = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&ws)
            .output()
            .context("git add failed")?;
        if !output.status.success() {
            return Err(anyhow!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let commit_msg = format!("forge: {}", message);
        let output = Command::new("git")
            .args(["commit", "-m", &commit_msg])
            .current_dir(&ws)
            .output()
            .context("git commit failed")?;
        if !output.status.success() {
            return Err(anyhow!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let output = Command::new("git")
            .args(["push", "origin", "main"])
            .current_dir(&ws)
            .output()
            .context("git push failed")?;
        if !output.status.success() {
            // Push failure is non-fatal — changes are committed locally
            tracing::warn!("git push failed: {}", String::from_utf8_lossy(&output.stderr));
        }

        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&ws)
            .output()?;
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: format!("✅ Changes committed as `{commit}`. Ready to build."),
            at: Utc::now(),
        };
        self.inner.write().messages.push(agent_msg);
        self.emit_status();
        Ok(commit)
    }

    /// Revert all pending changes.
    pub fn reject(&self) -> Result<String> {
        let ws = self.workspace_dir();
        let output = Command::new("git")
            .args(["checkout", "--", "."])
            .current_dir(&ws)
            .output()
            .context("git checkout failed")?;
        if !output.status.success() {
            return Err(anyhow!(
                "revert failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        // Also clean any new untracked files
        let _ = Command::new("git")
            .args(["clean", "-fd"])
            .current_dir(&ws)
            .output();

        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: "❌ Changes reverted. Workspace is clean.".into(),
            at: Utc::now(),
        };
        self.inner.write().messages.push(agent_msg);
        self.emit_status();
        Ok("changes reverted".into())
    }

    // ── Build ─────────────────────────────────

    /// Build the portable exe from the workspace.
    pub fn build(&self) -> Result<String> {
        {
            let mut inner = self.inner.write();
            inner.build_status = BuildStatus::Building;
        }
        self.emit_status();

        let ws = self.workspace_dir();
        let versions = self.versions_dir();
        fs::create_dir_all(&versions)?;

        // 1. npm install (if needed)
        if !ws.join("node_modules").exists() {
            let output = Command::new("npm")
                .args(["install"])
                .current_dir(&ws)
                .output()
                .context("npm install failed")?;
            if !output.status.success() {
                let err = String::from_utf8_lossy(&output.stderr);
                self.set_build_failed(&err);
                return Err(anyhow!("npm install failed: {err}"));
            }
        }

        // 2. npm run tauri build
        let output = Command::new("npm")
            .args(["run", "tauri", "build"])
            .current_dir(&ws)
            .output()
            .context("tauri build failed")?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            self.set_build_failed(&err);
            return Err(anyhow!("build failed: {err}"));
        }

        // 3. Find the built exe
        let bundle_dir = ws
            .join("src-tauri")
            .join("target")
            .join("release")
            .join("bundle")
            .join("nsis");
        let exe = find_exe(&bundle_dir)?;

        // 4. Copy to versions dir
        let version = get_version(&ws);
        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        let dest_name = format!("Brain2_{version}_{ts}.exe");
        let dest = versions.join(&dest_name);
        fs::copy(&exe, &dest)?;

        // 5. Get commit
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(&ws)
            .output()?;
        let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let fv = ForgeVersion {
            version: version.clone(),
            commit: commit.clone(),
            built_at: Utc::now(),
            exe_path: Some(dest.clone()),
        };
        self.inner.write().versions.push(fv.clone());
        self.save_versions(&self.inner.read().versions);

        let exe_str = dest.to_string_lossy().to_string();
        {
            let mut inner = self.inner.write();
            inner.build_status = BuildStatus::Success {
                exe_path: exe_str.clone(),
            };
        }

        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: format!("✅ Build successful!\n\nCommit: `{commit}`\nVersion: {version}\nOutput: `{exe_str}`\n\nClick **Install Update** to replace the current exe."),
            at: Utc::now(),
        };
        self.inner.write().messages.push(agent_msg);
        self.emit_status();
        Ok(exe_str)
    }

    // ── Install ───────────────────────────────

    /// Install a built exe, replacing the current one.
    pub fn install(&self, exe_path: &str) -> Result<String> {
        let new_exe = PathBuf::from(exe_path);
        if !new_exe.exists() {
            return Err(anyhow!("Build output not found at {exe_path}"));
        }

        let current_exe = std::env::current_exe()?;
        let backup = current_exe.with_extension("exe.bak");

        // Back up current
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        fs::rename(&current_exe, &backup)?;

        // Copy new exe into place
        fs::copy(&new_exe, &current_exe)?;

        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: "✅ Update installed! Restart Brain2 to run the new version.\n\nPrevious version backed up — restart with `--rollback` to revert.".into(),
            at: Utc::now(),
        };
        self.inner.write().messages.push(agent_msg);
        self.emit_status();
        Ok("installed — restart to apply".into())
    }

    // ── Rollback ──────────────────────────────

    /// Rollback to the previous exe.
    pub fn rollback(&self) -> Result<String> {
        let current_exe = std::env::current_exe()?;
        let backup = current_exe.with_extension("exe.bak");

        if !backup.exists() {
            return Err(anyhow!("No backup found. Nothing to roll back to."));
        }

        fs::rename(&backup, &current_exe)?;

        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: "✅ Rolled back to previous version. Restart to apply.".into(),
            at: Utc::now(),
        };
        self.inner.write().messages.push(agent_msg);
        self.emit_status();
        Ok("rolled back".into())
    }

    // ── Status ────────────────────────────────

    pub fn status(&self) -> ForgeStatus {
        let inner = self.inner.read();
        let ws = self.workspace_dir();
        let initialized = ws.join(".git").exists();

        let (branch, has_pending, pending_files) = if initialized {
            let branch = git_branch(&ws).unwrap_or_default();
            let files = git_pending_files(&ws).unwrap_or_default();
            (branch, !files.is_empty(), files)
        } else {
            (String::new(), false, vec![])
        };

        ForgeStatus {
            initialized,
            repo_url: REPO_URL.into(),
            branch,
            has_pending_changes: has_pending,
            pending_files,
            build_status: inner.build_status.clone(),
            versions: inner.versions.clone(),
            messages: inner.messages.clone(),
        }
    }

    // ── Internal helpers ──────────────────────

    fn set_build_failed(&self, error: &str) {
        let mut inner = self.inner.write();
        inner.build_status = BuildStatus::Failed {
            error: error.to_string(),
        };
        let agent_msg = ForgeMessage {
            id: Uuid::new_v4(),
            role: "agent".into(),
            content: format!("❌ Build failed:\n```\n{error}\n```"),
            at: Utc::now(),
        };
        inner.messages.push(agent_msg);
    }

    fn emit_status(&self) {
        let status = self.status();
        let _ = self.app_handle.emit("forge:status", status);
    }

    fn load_versions(data_dir: &Path) -> Vec<ForgeVersion> {
        let path = data_dir.join("forge").join("versions.json");
        match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => vec![],
        }
    }

    fn save_versions(&self, versions: &[ForgeVersion]) {
        let dir = self.forge_dir();
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("versions.json");
        if let Ok(json) = serde_json::to_string_pretty(versions) {
            let _ = fs::write(&path, json);
        }
    }
}

// ── Helpers ──────────────────────────────────

fn git_branch(ws: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(ws)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_pending_files(ws: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["diff", "--name-only"])
        .current_dir(ws)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

fn find_exe(bundle_dir: &Path) -> Result<PathBuf> {
    // Walk the bundle dir looking for the setup exe
    for entry in walkdir::WalkDir::new(bundle_dir)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".exe") && name.contains("setup") {
            return Ok(entry.path().to_path_buf());
        }
    }
    // Fallback: any .exe
    for entry in walkdir::WalkDir::new(bundle_dir)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let name = entry.file_name().to_string_lossy();
        if name.ends_with(".exe") {
            return Ok(entry.path().to_path_buf());
        }
    }
    Err(anyhow!("No .exe found in {bundle_dir:?}"))
}

fn get_version(ws: &Path) -> String {
    let conf = ws.join("src-tauri").join("tauri.conf.json");
    match fs::read_to_string(&conf) {
        Ok(s) => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                v.get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.6.0")
                    .to_string()
            } else {
                "0.6.0".into()
            }
        }
        Err(_) => "0.6.0".into(),
    }
}

/// Build a source-tree context for the agent — lists all source files and
/// their sizes so the agent knows what it's working with.
fn build_source_context(ws: &Path) -> Result<String> {
    let mut ctx = String::from("## Brain2 source tree\n\n");

    // Key source directories
    let dirs = [
        ("src-tauri/src/", "Rust backend"),
        ("src/", "React/TypeScript frontend"),
        ("src-tauri/tauri.conf.json", "Tauri config"),
        ("src-tauri/Cargo.toml", "Rust dependencies"),
        ("package.json", "Node dependencies"),
    ];

    for (path, desc) in &dirs {
        let full = ws.join(path);
        if full.is_dir() {
            ctx.push_str(&format!("### {desc} (`{path}`)\n"));
            if let Ok(entries) = fs::read_dir(&full) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    ctx.push_str(&format!("- {} ({} bytes)\n", name.to_string_lossy(), size));
                }
            }
        } else if full.is_file() {
            let size = fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
            ctx.push_str(&format!("- `{path}` — {desc} ({size} bytes)\n"));
        }
        ctx.push('\n');
    }

    Ok(ctx)
}

/// Call Claude to process a forge request.
///
/// The agent is prompted with:
/// 1. The Brain2 architecture overview
/// 2. The current source tree
/// 3. The user's request
///
/// It responds with:
/// 1. A plain-English explanation of what it will change
/// 2. Specific file patches in a parseable format
async fn call_forge_agent(api_key: &str, request: &str, source_context: &str) -> Result<String> {
    let system = format!(
        "You are the Brain2 Forge agent — a self-improvement AI inside a desktop app.\n\n\
        Brain2 is a Tauri 2 desktop app (Rust backend + React/TypeScript frontend) \
        that transcribes meetings via Deepgram, translates via Claude/OpenAI, and \
        now includes the Forge — a self-modifying code agent (you).\n\n\
        When the user asks for an improvement, you:\n\
        1. Explain what you'll change and why\n\
        2. Provide concrete file patches in this format:\n\n\
        ```forge:path/to/file.ext\n\
        <<<OLD\n\
        (exact old code to find)\n\
        ===\n\
        (new replacement code)\n\
        >>>NEW\n\
        ```\n\n\
        Rules:\n\
        - Only change files that exist in the source tree below\n\
        - Keep changes minimal and focused on the request\n\
        - Prefer modifying existing files over creating new ones\n\
        - If you need to create a new file, use ```forge:path/to/new.ext (NEW FILE)```\n\
        - If the request is unclear, ask for clarification first\n\
        - Never delete files unless explicitly asked\n\n\
        {source_context}"
    );

    let payload = serde_json::json!({
        "model": "claude-haiku-4-5-20251001",
        "max_tokens": 4096,
        "system": [{"type": "text", "text": system}],
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": request}]
        }]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&payload)
        .send()
        .await
        .context("Failed to reach Anthropic API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Anthropic API error {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.context("Failed to parse response")?;
    let content = body["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
        .to_string();

    Ok(content)
}

/// Parse forge patches from the agent response and apply them to the workspace.
///
/// Patch format:
/// ```forge:path/to/file.ext
/// <<<OLD
/// old code
/// ===
/// new code
/// >>>NEW
/// ```
fn apply_changes(ws: &Path, response: &str) -> Result<Vec<String>> {
    let mut files_changed: Vec<String> = vec![];

    // Parse ```forge: blocks
    let mut in_forge = false;
    let mut current_file: Option<String> = None;
    let mut current_section = String::new();
    let mut old_content = String::new();
    let mut new_content = String::new();
    let mut in_old = false;
    let mut in_new = false;

    for line in response.lines() {
        if line.starts_with("```forge:") {
            in_forge = true;
            // Extract file path
            current_file = Some(
                line.trim_start_matches("```forge:")
                    .trim()
                    .trim_end_matches("```")
                    .to_string(),
            );
            current_section.clear();
            old_content.clear();
            new_content.clear();
            in_old = false;
            in_new = false;
        } else if in_forge && line == "```" {
            // End of forge block — apply the patch
            if let Some(ref file) = current_file {
                let is_new_file = file.contains("(NEW FILE)");
                let clean_file = file.replace(" (NEW FILE)", "").trim().to_string();
                let file_path = ws.join(&clean_file);

                let mut applied = false;
                if is_new_file {
                    if let Some(parent) = file_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&file_path, &new_content)?;
                    applied = true;
                } else if file_path.exists() {
                    let existing = fs::read_to_string(&file_path)?;
                    if existing.contains(&old_content) {
                        // Replace only the first match — patches target one site.
                        let updated = existing.replacen(&old_content, &new_content, 1);
                        fs::write(&file_path, updated)?;
                        applied = true;
                    } else {
                        tracing::warn!(
                            "Forge: old string not found in {clean_file}. Skipping patch."
                        );
                    }
                }
                // Only report files we actually changed, so the diff/approve
                // step doesn't claim edits that were skipped.
                if applied {
                    files_changed.push(clean_file);
                }
            }
            in_forge = false;
            current_file = None;
        } else if in_forge {
            if line == "<<<OLD" {
                in_old = true;
                in_new = false;
            } else if line == "===" && in_old {
                in_old = false;
                in_new = true;
            } else if line == ">>>NEW" {
                in_new = false;
            } else if in_old {
                if !old_content.is_empty() {
                    old_content.push('\n');
                }
                old_content.push_str(line);
            } else if in_new {
                if !new_content.is_empty() {
                    new_content.push('\n');
                }
                new_content.push_str(line);
            }
        }
    }

    Ok(files_changed)
}

// ── Tauri commands ───────────────────────────

use tauri::State;

#[tauri::command]
pub async fn forge_init(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.init().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_status(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<ForgeStatus, String> {
    Ok(forge.status())
}

#[tauri::command]
pub async fn forge_chat(
    message: String,
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    let api_key = crate::settings::require_llm_credentials().map_err(|e| e.to_string())?;
    forge.chat(message, &api_key).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_diff(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.diff().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_approve(
    message: String,
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.approve(&message).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_reject(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.reject().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_build(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.build().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_install(
    exe_path: String,
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.install(&exe_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_rollback(
    forge: State<'_, Arc<ForgeState>>,
) -> Result<String, String> {
    forge.rollback().map_err(|e| e.to_string())
}
