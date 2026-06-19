//! Local Whisper STT engine (feature `local-stt`).
//!
//! Mirrors `deepgram::run`: it consumes the meeting's 16 kHz mono Int16 PCM
//! stream and emits the SAME `DeepgramEvent`s, so `handle_dg_event`, the
//! translate/Brain/cost paths, and the whole frontend keep working unchanged.
//!
//! Whisper is not a streaming model, so "live" is faked with VAD-gated
//! chunking: accumulate speech, and when the speaker pauses (or an utterance
//! gets long), transcribe that complete utterance once on a blocking thread and
//! emit it as a final segment. Committed text lands ~1-3 s after a pause —
//! the tradeoff vs Deepgram's sub-second interims (see the Settings note).

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::deepgram::DeepgramEvent;

pub struct LocalSttConfig {
    pub model_path: std::path::PathBuf,
    /// GPU device index. 1 = Intel Arc 140T (0 is the NVIDIA dGPU we avoid).
    pub gpu_device: i32,
    /// Deepgram-style source language code; "multi" => Whisper auto-detect.
    pub language: String,
}

const SAMPLE_RATE: usize = 16_000;
const FRAME: usize = 1600; // 100 ms @ 16 kHz
/// ~1 s of silence after speech ends an utterance. Longer than 700 ms so
/// natural sentences aren't chopped into sub-second fragments — short chunks
/// wreck Whisper's per-utterance language detection.
const SILENCE_FRAMES_FLUSH: usize = 10;
/// Safety flush so a long monologue still commits periodically.
const MAX_UTTERANCE_SAMPLES: usize = SAMPLE_RATE * 20;
/// Ignore sub-second blips (coughs, clicks) and give Whisper enough audio to
/// detect language reliably (it misdetects badly on half-second chunks).
const MIN_UTTERANCE_SAMPLES: usize = SAMPLE_RATE;
/// Normalized-RMS speech/silence threshold (tunable; mic-gain dependent).
const RMS_THRESHOLD: f32 = 0.012;
/// Emit a cost Stats event roughly every 5 s of audio.
const STATS_EVERY_BYTES: u64 = 32_000 * 5;

pub async fn run(
    cfg: LocalSttConfig,
    mut audio_rx: mpsc::Receiver<Bytes>,
    out: mpsc::Sender<DeepgramEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    // Load the model on a blocking thread (Vulkan init is ~2 s).
    let model_path = cfg.model_path.to_string_lossy().to_string();
    let gpu = cfg.gpu_device;
    let ctx = tokio::task::spawn_blocking(move || -> Result<WhisperContext> {
        let mut p = WhisperContextParameters::default();
        p.use_gpu(true);
        p.gpu_device(gpu);
        WhisperContext::new_with_params(&model_path, p).map_err(|e| anyhow!("load whisper model: {e}"))
    })
    .await
    .context("whisper load task")??;
    let ctx = Arc::new(ctx);
    tracing::info!(gpu, "local whisper model loaded");

    let mut buf: Vec<f32> = Vec::with_capacity(SAMPLE_RATE * 30);
    let mut frame: Vec<f32> = Vec::with_capacity(FRAME);
    let mut had_speech = false;
    let mut silence_frames = 0usize;
    let mut bytes_since_stat: u64 = 0;

    // Language handling, derived from the source-language setting:
    //   "multi"/empty   → unconstrained auto-detect (any of Whisper's 99 langs)
    //   single ("en")   → pinned to that language
    //   list ("en,nl")  → auto-detect, but CONSTRAINED to those languages: if
    //                     Whisper guesses anything else (the random Portuguese/
    //                     Polish garbage on a mixed meeting) the chunk is
    //                     re-decoded as the meeting's sticky in-set language.
    let allowed: Vec<String> = cfg
        .language
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty() && s != "multi")
        .collect();
    let mut sticky_lang: Option<String> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            chunk = audio_rx.recv() => {
                let Some(bytes) = chunk else { break };
                bytes_since_stat += bytes.len() as u64;
                for pair in bytes.chunks_exact(2) {
                    let s = i16::from_le_bytes([pair[0], pair[1]]);
                    frame.push(s as f32 / 32768.0);
                    if frame.len() < FRAME {
                        continue;
                    }
                    let speaking = rms(&frame) > RMS_THRESHOLD;
                    if speaking {
                        had_speech = true;
                        silence_frames = 0;
                        buf.extend_from_slice(&frame);
                    } else if had_speech {
                        // Keep a little trailing silence for context, then flush.
                        silence_frames += 1;
                        buf.extend_from_slice(&frame);
                        if silence_frames >= SILENCE_FRAMES_FLUSH {
                            transcribe_and_emit(&ctx, &mut buf, &out, &allowed, &mut sticky_lang).await;
                            had_speech = false;
                            silence_frames = 0;
                        }
                    }
                    // (silence before any speech is dropped)
                    frame.clear();
                    if buf.len() >= MAX_UTTERANCE_SAMPLES {
                        transcribe_and_emit(&ctx, &mut buf, &cfg, &out).await;
                        had_speech = false;
                        silence_frames = 0;
                    }
                }
                if bytes_since_stat >= STATS_EVERY_BYTES {
                    let _ = out
                        .send(DeepgramEvent::Stats { bytes_since_last: bytes_since_stat })
                        .await;
                    bytes_since_stat = 0;
                }
            }
        }
    }

    // Flush whatever is left when the meeting stops.
    if !buf.is_empty() {
        transcribe_and_emit(&ctx, &mut buf, &cfg, &out).await;
    }
    let _ = out.send(DeepgramEvent::Closed).await;
    Ok(())
}

fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    let sum: f32 = frame.iter().map(|x| x * x).sum();
    (sum / frame.len() as f32).sqrt()
}

/// Transcribe the buffered utterance on a blocking thread and emit it as a
/// final segment. Takes the buffer (leaving it empty for the next utterance).
///
/// `allowed` (from the source-language setting) controls language handling:
/// empty = unconstrained auto-detect; one entry = pinned; ≥2 entries = auto but
/// constrained to that set. `sticky` carries the last in-set language across
/// calls so an out-of-set mis-detection re-decodes to the language actually
/// being spoken.
async fn transcribe_and_emit(
    ctx: &Arc<WhisperContext>,
    buf: &mut Vec<f32>,
    out: &mpsc::Sender<DeepgramEvent>,
    allowed: &[String],
    sticky: &mut Option<String>,
) {
    let samples = std::mem::take(buf);
    if samples.len() < MIN_UTTERANCE_SAMPLES {
        return;
    }
    let secs = samples.len() as f64 / SAMPLE_RATE as f64;
    let ctx = ctx.clone();
    let allowed_v = allowed.to_vec();
    let sticky_in = sticky.clone();

    let result = tokio::task::spawn_blocking(move || -> Result<(String, Option<String>)> {
        // One decode pass. `force` = a language code, or "auto" to let Whisper
        // detect. Anti-hallucination params (no-context, temp 0, suppress blank/
        // non-speech) keep short VAD chunks from becoming foreign-language
        // garbage like "Chuj chucha".
        let decode = |force: &str| -> Result<(String, String)> {
            let mut state = ctx.create_state().map_err(|e| anyhow!("create state: {e}"))?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_language(Some(force));
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_no_context(true);
            params.set_temperature(0.0);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);
            state.full(params, &samples).map_err(|e| anyhow!("transcribe: {e}"))?;
            let n = state.full_n_segments();
            let mut text = String::new();
            for i in 0..n {
                if let Some(seg) = state.get_segment(i) {
                    if let Ok(s) = seg.to_str_lossy() {
                        text.push_str(&s);
                    }
                }
            }
            let det = if force == "auto" {
                whisper_rs::get_lang_str(state.full_lang_id_from_state())
                    .unwrap_or("")
                    .to_string()
            } else {
                force.to_string()
            };
            Ok((text.trim().to_string(), det))
        };

        let pinned = allowed_v.len() == 1;
        let constrained = allowed_v.len() >= 2;
        let first = if pinned { allowed_v[0].as_str() } else { "auto" };
        let (mut text, mut det) = decode(first)?;

        // Constrained: if Whisper picked a language outside the allowed set it
        // almost certainly mis-detected — re-decode forcing the sticky in-set
        // language (or the first allowed one).
        if constrained && !allowed_v.iter().any(|a| det.starts_with(a.as_str())) {
            let forced = sticky_in
                .filter(|s| allowed_v.iter().any(|a| s.starts_with(a.as_str())))
                .unwrap_or_else(|| allowed_v[0].clone());
            let (t2, d2) = decode(&forced)?;
            text = t2;
            det = d2;
        }

        let detected = if det.is_empty() { None } else { Some(det) };
        Ok((text, detected))
    })
    .await;

    match result {
        Ok(Ok((text, detected))) if !text.is_empty() => {
            // Remember the last in-set language so a later mis-detect re-decodes
            // to what's actually being spoken.
            if let Some(d) = &detected {
                if allowed.is_empty() || allowed.iter().any(|a| d.starts_with(a.as_str())) {
                    *sticky = Some(d.clone());
                }
            }
            let _ = out
                .send(DeepgramEvent::Final {
                    text,
                    start: 0.0,
                    duration: secs,
                    speech_final: true,
                    language: detected,
                    speaker: None,
                })
                .await;
        }
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::warn!(?e, "local stt transcribe failed"),
        Err(e) => tracing::warn!(?e, "local stt task join failed"),
    }
}
