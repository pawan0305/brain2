//! Windows audio capture.
//!
//! Replaces the macOS Swift sidecar (ScreenCaptureKit + AVAudioEngine) with
//! an in-process WASAPI implementation:
//!
//!   * System audio  → WASAPI **loopback** on the default render endpoint
//!     (captures everything other apps are playing: Teams, Zoom, browser …).
//!   * Microphone     → WASAPI capture on the default capture endpoint.
//!
//! Each source is converted to mono, resampled to 16 kHz, and pushed into a
//! small ring buffer. A mixer pump drains both rings every 100 ms, sums them
//! sample-aligned (clamped to Int16), and forwards a single coherent stream
//! of 16 kHz mono Int16 LE PCM — the exact format Deepgram expects — over an
//! mpsc channel. It also emits `audio:level` events (~5 Hz) for the top-bar
//! VU meter, the same way the old sidecar's `META:level` lines did.
//!
//! Mixing both sources sample-by-sample (rather than concatenating whichever
//! arrived first) is what prevents "everything appears twice in the
//! transcript" when mic sidetone bleeds into the system loopback.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

const OUTPUT_SAMPLE_RATE: u32 = 16_000;
/// 100 ms at 16 kHz — one mixer tick's worth of output samples.
const TICK_FRAMES: usize = 1_600;
/// Cap each source ring at ~500 ms so a stalled mixer can't grow it unbounded.
const MAX_BUFFERED: usize = 8_000;
/// `AUDCLNT_BUFFERFLAGS_SILENT` — the packet is all zeros; the data pointer is
/// not meaningful, so we just advance the frame count.
const BUFFERFLAGS_SILENT: u32 = 0x2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Mic,
    Sys,
}

#[derive(Default)]
struct MixState {
    mic: Vec<i16>,
    sys: Vec<i16>,
}

/// Spawn the WASAPI capture + mixer threads. Returns a channel of raw 16 kHz
/// mono Int16 LE PCM bytes — identical contract to the old Swift sidecar, so
/// the meeting orchestrator (`commands::run_meeting`) is unchanged.
///
/// `cancel` terminates every thread when the meeting stops; the threads poll
/// it and exit on their next loop iteration.
pub async fn start_capture(
    app: &AppHandle,
    cancel: CancellationToken,
    include_mic: bool,
) -> Result<mpsc::Receiver<Bytes>> {
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    let mixer = Arc::new(Mutex::new(MixState::default()));

    // System audio (loopback on the default render endpoint).
    {
        let mixer = mixer.clone();
        let cancel = cancel.clone();
        let app = app.clone();
        std::thread::Builder::new()
            .name("wasapi-sys".into())
            .spawn(move || {
                if let Err(e) = run_capture(Source::Sys, true, mixer, cancel) {
                    tracing::error!(error = %e, "system audio (loopback) capture failed");
                    let _ = app.emit(
                        "error",
                        serde_json::json!({
                            "message": format!(
                                "System audio capture failed: {e}. Make sure an audio output device is enabled."
                            )
                        }),
                    );
                }
            })?;
    }

    // Microphone (default capture endpoint). Optional — a missing mic is not
    // fatal; the meeting still runs on system audio alone.
    if include_mic {
        let mixer = mixer.clone();
        let cancel = cancel.clone();
        std::thread::Builder::new()
            .name("wasapi-mic".into())
            .spawn(move || {
                if let Err(e) = run_capture(Source::Mic, false, mixer, cancel) {
                    tracing::warn!(error = %e, "mic capture failed (continuing without mic)");
                }
            })?;
    }

    // Mixer pump: paces output at a steady 16 kHz regardless of how either
    // source produces data, and emits VU levels.
    {
        let mixer = mixer.clone();
        let cancel = cancel.clone();
        let app = app.clone();
        std::thread::Builder::new()
            .name("wasapi-mix".into())
            .spawn(move || mixer_pump(mixer, tx, cancel, app))?;
    }

    Ok(rx)
}

/// Open a WASAPI client on the default endpoint for `source`, in loopback mode
/// when `loopback` is set, and pump audio into the mixer until `cancel` fires.
fn run_capture(
    source: Source,
    loopback: bool,
    mixer: Arc<Mutex<MixState>>,
    cancel: CancellationToken,
) -> windows::core::Result<()> {
    unsafe {
        // Each capture thread owns its own COM apartment (MTA) so the audio
        // interfaces are created and used on a single thread — no marshaling.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = capture_loop(source, loopback, &mixer, &cancel);
        CoUninitialize();
        result
    }
}

unsafe fn capture_loop(
    source: Source,
    loopback: bool,
    mixer: &Mutex<MixState>,
    cancel: &CancellationToken,
) -> windows::core::Result<()> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

    // Loopback captures what plays *out* of the render endpoint, so even for
    // system audio we open the render device — just with the LOOPBACK flag.
    let dataflow = if loopback { eRender } else { eCapture };
    let device = enumerator.GetDefaultAudioEndpoint(dataflow, eConsole)?;
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;

    // Shared mode hands us the device's mix format (usually 32-bit float,
    // 2 ch, 48 kHz). We read its fields, then resample to 16 kHz mono.
    let pwfx = client.GetMixFormat()?;
    let wfx = &*pwfx;
    let in_rate = wfx.nSamplesPerSec;
    let channels = wfx.nChannels as usize;
    let bits = wfx.wBitsPerSample as usize;
    let block_align = wfx.nBlockAlign as usize;
    let is_float = sample_is_float(pwfx);

    let stream_flags = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    // 200 ms buffer (100-ns units). For shared mode the periodicity must be 0.
    let init = client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        stream_flags,
        2_000_000,
        0,
        pwfx,
        None,
    );
    // The format struct was allocated by GetMixFormat; Initialize has now
    // copied what it needs, so release it regardless of the outcome.
    CoTaskMemFree(Some(pwfx as *const c_void));
    init?;

    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let mut resampler = Resampler::new(in_rate);
    let mut scratch: Vec<i16> = Vec::with_capacity(TICK_FRAMES * 2);

    while !cancel.is_cancelled() {
        // Drain every queued packet, then sleep briefly. Loopback delivers no
        // packets while the system is silent — that's fine, the mixer pads.
        loop {
            let packet = capture.GetNextPacketSize()?;
            if packet == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;

            if frames > 0 {
                scratch.clear();
                if (flags & BUFFERFLAGS_SILENT) != 0 || data.is_null() {
                    // Emit silence so timing stays aligned with real audio.
                    for _ in 0..frames {
                        resampler.push(0.0, &mut scratch);
                    }
                } else {
                    let byte_len = frames as usize * block_align;
                    let raw = std::slice::from_raw_parts(data, byte_len);
                    for f in 0..frames as usize {
                        let frame = &raw[f * block_align..f * block_align + block_align];
                        let mono = frame_to_mono(frame, channels, bits, is_float);
                        resampler.push(mono, &mut scratch);
                    }
                }
                push_samples(mixer, source, &scratch);
            }

            capture.ReleaseBuffer(frames)?;
        }

        std::thread::sleep(Duration::from_millis(8));
    }

    let _ = client.Stop();
    Ok(())
}

/// Average all channels of one interleaved frame into a single mono sample in
/// the range [-1.0, 1.0]. Reads via `from_le_bytes` so we never assume the
/// WASAPI buffer is aligned for `f32`/`i32`.
#[inline]
fn frame_to_mono(frame: &[u8], channels: usize, bits: usize, is_float: bool) -> f32 {
    if channels == 0 {
        return 0.0;
    }
    let bytes_per = bits / 8;
    let mut acc = 0.0f32;
    for ch in 0..channels {
        let off = ch * bytes_per;
        if off + bytes_per > frame.len() {
            break;
        }
        let s = &frame[off..off + bytes_per];
        let v = if is_float && bytes_per == 4 {
            f32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            match bytes_per {
                2 => i16::from_le_bytes([s[0], s[1]]) as f32 / 32_768.0,
                4 => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                3 => {
                    // 24-bit signed little-endian, sign-extended to i32.
                    let raw = (s[0] as i32) | ((s[1] as i32) << 8) | ((s[2] as i32) << 16);
                    let signed = (raw << 8) >> 8;
                    signed as f32 / 8_388_608.0
                }
                1 => (s[0] as f32 - 128.0) / 128.0,
                _ => 0.0,
            }
        };
        acc += v;
    }
    acc / channels as f32
}

/// Is the mix format IEEE float? Handles both the plain `WAVE_FORMAT_IEEE_FLOAT`
/// tag and `WAVE_FORMAT_EXTENSIBLE` (where the real type is the SubFormat GUID,
/// whose `Data1` is 3 for float / 1 for PCM).
unsafe fn sample_is_float(pwfx: *const WAVEFORMATEX) -> bool {
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    let tag = (*pwfx).wFormatTag;
    if tag == WAVE_FORMAT_IEEE_FLOAT {
        return true;
    }
    if tag == WAVE_FORMAT_EXTENSIBLE {
        let ext = &*(pwfx as *const WAVEFORMATEXTENSIBLE);
        return ext.SubFormat.data1 == 3;
    }
    false
}

fn push_samples(mixer: &Mutex<MixState>, source: Source, samples: &[i16]) {
    if samples.is_empty() {
        return;
    }
    let mut g = mixer.lock().unwrap();
    let buf = match source {
        Source::Mic => &mut g.mic,
        Source::Sys => &mut g.sys,
    };
    buf.extend_from_slice(samples);
    if buf.len() > MAX_BUFFERED {
        let excess = buf.len() - MAX_BUFFERED;
        buf.drain(0..excess);
    }
}

/// Drain both source rings every 100 ms, sum sample-aligned, ship a single
/// 16 kHz mono Int16 LE chunk, and emit VU levels ~5×/sec.
fn mixer_pump(
    mixer: Arc<Mutex<MixState>>,
    tx: mpsc::Sender<Bytes>,
    cancel: CancellationToken,
    app: AppHandle,
) {
    let mut ticks: u64 = 0;
    while !cancel.is_cancelled() {
        std::thread::sleep(Duration::from_millis(100));

        let mut mic = vec![0i16; TICK_FRAMES];
        let mut sys = vec![0i16; TICK_FRAMES];
        let (mic_n, sys_n) = {
            let mut g = mixer.lock().unwrap();
            let mic_n = g.mic.len().min(TICK_FRAMES);
            for (i, s) in g.mic.drain(0..mic_n).enumerate() {
                mic[i] = s;
            }
            let sys_n = g.sys.len().min(TICK_FRAMES);
            for (i, s) in g.sys.drain(0..sys_n).enumerate() {
                sys[i] = s;
            }
            (mic_n, sys_n)
        };

        let mut bytes = Vec::with_capacity(TICK_FRAMES * 2);
        for i in 0..TICK_FRAMES {
            let mixed =
                (mic[i] as i32 + sys[i] as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            bytes.extend_from_slice(&mixed.to_le_bytes());
        }
        if tx.blocking_send(Bytes::from(bytes)).is_err() {
            break; // receiver dropped — meeting ended
        }

        ticks += 1;
        if ticks % 2 == 0 {
            let mic_rms = rms(&mic[..mic_n]);
            let sys_rms = rms(&sys[..sys_n]);
            let _ = app.emit(
                "audio:level",
                serde_json::json!({ "mic": mic_rms, "sys": sys_rms }),
            );
        }
    }
}

fn rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut acc = 0.0f64;
    for &s in samples {
        let v = s as f64 / 32_768.0;
        acc += v * v;
    }
    (acc / samples.len() as f64).sqrt()
}

/// Streaming linear-interpolation resampler from `in_rate` to 16 kHz mono.
/// Linear interpolation is plenty for speech headed to an STT engine, and it
/// handles any device rate (48 kHz, 44.1 kHz, …) without extra dependencies.
struct Resampler {
    /// Input samples consumed per output sample (in_rate / 16000).
    step: f64,
    /// Output cursor position within the current [prev, cur] input segment.
    pos: f64,
    prev: f32,
    has_prev: bool,
}

impl Resampler {
    fn new(in_rate: u32) -> Self {
        Self {
            step: in_rate as f64 / OUTPUT_SAMPLE_RATE as f64,
            pos: 0.0,
            prev: 0.0,
            has_prev: false,
        }
    }

    #[inline]
    fn push(&mut self, cur: f32, out: &mut Vec<i16>) {
        if !self.has_prev {
            self.prev = cur;
            self.has_prev = true;
            return;
        }
        // Emit every output sample whose position falls in [0, 1) of the
        // segment between the previous and current input sample.
        while self.pos < 1.0 {
            let f = self.pos as f32;
            let v = self.prev + (cur - self.prev) * f;
            out.push(clamp_i16(v));
            self.pos += self.step;
        }
        self.pos -= 1.0;
        self.prev = cur;
    }
}

#[inline]
fn clamp_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_passthrough_16k() {
        // Equal in/out rate: one output per input (minus the priming sample),
        // value preserved.
        let mut r = Resampler::new(16_000);
        let mut out = Vec::new();
        for _ in 0..1000 {
            r.push(0.5, &mut out);
        }
        assert_eq!(out.len(), 999);
        assert!(out.iter().all(|&s| s == 16_384));
    }

    #[test]
    fn resampler_48k_to_16k_decimates_3x() {
        // 48 kHz → 16 kHz is exactly 3:1, so 3000 inputs → 1000 outputs.
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        for _ in 0..3000 {
            r.push(0.25, &mut out);
        }
        assert_eq!(out.len(), 1000);
    }

    #[test]
    fn resampler_44k_to_16k_ratio() {
        // Non-integer ratio (44100/16000 ≈ 2.756): output length is within a
        // sample of the ideal, and nothing panics.
        let mut r = Resampler::new(44_100);
        let mut out = Vec::new();
        for _ in 0..44_100 {
            r.push(0.1, &mut out);
        }
        let expected = (44_100.0 / (44_100.0 / 16_000.0)) as i64; // == 16000
        assert!((out.len() as i64 - expected).abs() <= 2, "len = {}", out.len());
    }

    #[test]
    fn mono_from_float_stereo_averages() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&1.0f32.to_le_bytes());
        frame.extend_from_slice(&(-1.0f32).to_le_bytes());
        assert!((frame_to_mono(&frame, 2, 32, true) - 0.0).abs() < 1e-6);

        let mut frame2 = Vec::new();
        frame2.extend_from_slice(&0.5f32.to_le_bytes());
        frame2.extend_from_slice(&0.5f32.to_le_bytes());
        assert!((frame_to_mono(&frame2, 2, 32, true) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mono_from_i16_pcm() {
        // 16384 / 32768 = 0.5
        let frame = 16_384i16.to_le_bytes();
        assert!((frame_to_mono(&frame, 1, 16, false) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn mix_saturates() {
        let mixed =
            (20_000i32 + 20_000).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        assert_eq!(mixed, i16::MAX);
        let mixed_neg =
            (-20_000i32 - 20_000).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        assert_eq!(mixed_neg, i16::MIN);
    }

    #[test]
    fn rms_known_values() {
        assert_eq!(rms(&[]), 0.0);
        let half = 16_384i16; // ≈ 0.5 full scale
        let r = rms(&[half, half, half, half]);
        assert!((r - 0.5).abs() < 1e-3, "rms = {r}");
    }

    #[test]
    fn clamp_i16_bounds() {
        assert_eq!(clamp_i16(2.0), 32_767);
        assert_eq!(clamp_i16(-2.0), -32_767);
        assert_eq!(clamp_i16(0.0), 0);
    }
}
