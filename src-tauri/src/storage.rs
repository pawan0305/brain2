use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::Meeting;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSummaryRow {
    pub id: Uuid,
    pub title: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub ended_at: Option<chrono::DateTime<chrono::Utc>>,
    pub segment_count: usize,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub fn ensure_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create dir {dir:?}"))
}

pub fn save_meeting(dir: &Path, meeting: &Meeting) -> Result<()> {
    ensure_dir(dir)?;
    let path = meeting_path(dir, meeting.id);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(meeting).context("serialize meeting")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {tmp:?}"))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {path:?}"))?;
    Ok(())
}

pub fn load_meeting(dir: &Path, id: Uuid) -> Result<Meeting> {
    let path = meeting_path(dir, id);
    let bytes = std::fs::read(&path).with_context(|| format!("read {path:?}"))?;
    serde_json::from_slice(&bytes).context("deserialize meeting")
}

pub fn delete_meeting(dir: &Path, id: Uuid) -> Result<()> {
    let path = meeting_path(dir, id);
    if path.exists() {
        std::fs::remove_file(&path).with_context(|| format!("remove {path:?}"))?;
    }
    Ok(())
}

pub fn list_meetings(dir: &Path) -> Result<Vec<MeetingSummaryRow>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {dir:?}"))? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let bytes = match std::fs::read(&p) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(?err, ?p, "skip unreadable meeting file");
                continue;
            }
        };
        match serde_json::from_slice::<Meeting>(&bytes) {
            Ok(m) => rows.push(MeetingSummaryRow {
                id: m.id,
                title: m.title,
                started_at: m.started_at,
                ended_at: m.ended_at,
                segment_count: m.segments.len(),
                tags: m.tags,
            }),
            Err(err) => {
                tracing::warn!(?err, ?p, "skip malformed meeting file");
            }
        }
    }
    rows.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(rows)
}

fn meeting_path(dir: &Path, id: Uuid) -> PathBuf {
    dir.join(format!("{id}.json"))
}
