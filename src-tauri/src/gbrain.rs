//! Retrieval from the user's local **gbrain** knowledge base (lives in WSL).
//!
//! gbrain is a personal "knowledge brain" — an embedded vector DB
//! (`~/.gbrain/brain.pglite`) that a Hermes cron job keeps fresh by
//! re-importing the user's Knowledge folder every 30 min (incremental: only
//! new/changed files) and embedding new chunks **locally via Ollama**. It is
//! exactly the "sweep once, then only ingest what's new" brain we want — so
//! Brain2 reuses it as the retrieval layer for "Ask the meeting" instead of
//! cold-sweeping the filesystem on every single question.
//!
//! Everything here runs **on-device**: the query embedding + vector search both
//! happen in WSL against the local Postgres/Ollama. The only thing that later
//! leaves the machine is whatever the chosen synthesis engine (Claude) is sent.

use std::process::Stdio;

use anyhow::{anyhow, Context, Result};

/// One WSL round-trip: run a hybrid search for the question, print the top hits
/// (slug + snippet), then pull the full markdown of the strongest few pages.
/// Producing the whole context block in a single `bash -lc` avoids paying the
/// WSL/bun cold-start once per `gbrain` sub-call.
///
/// `$BRAIN2_GBRAIN_Q` is injected from the host env (forwarded via `WSLENV`) so
/// the question never has to survive bash quoting.
const RETRIEVE_SCRIPT: &str = r#"
export PATH=$HOME/.bun/bin:/usr/local/bin:/usr/bin:/bin
HITS=$(gbrain query "$BRAIN2_GBRAIN_Q" --no-expand 2>/dev/null)
[ -z "$HITS" ] && exit 0
echo "Top matches:"
echo "$HITS" | head -8
echo
echo "Full text of the most relevant pages:"
echo "$HITS" | grep -oP '^\[[0-9.]+\]\s+\K\S+' | awk '!seen[$0]++' | head -4 | while read -r slug; do
  echo "=== $slug ==="
  gbrain get "$slug" 2>/dev/null
  echo
done
"#;

/// Retrieve a prompt-ready knowledge block relevant to `question` from gbrain.
/// Returns an empty string if gbrain has no hits, and an error if WSL/gbrain is
/// unavailable — callers treat both as "no knowledge" and fall back.
pub async fn retrieve(question: &str) -> Result<String> {
    let out = wsl(RETRIEVE_SCRIPT, ("BRAIN2_GBRAIN_Q", question)).await?;
    Ok(out.trim().to_string())
}

async fn wsl(script: &str, env: (&str, &str)) -> Result<String> {
    let mut cmd = crate::proc::command("wsl");
    cmd.arg("--").arg("bash").arg("-lc").arg(script);
    cmd.env(env.0, env.1);
    cmd.env("WSLENV", format!("{}/u", env.0));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let out = cmd
        .output()
        .await
        .context("failed to query gbrain — is WSL installed and `gbrain` set up inside it?")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gbrain query failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}
