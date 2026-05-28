# OneTrueDutchie (Windows)

**Real-time meeting transcription, translation, and AI assistant for Windows.**

[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=for-the-badge)](LICENSE)
[![Windows 10/11](https://img.shields.io/badge/Windows-10%20%2F%2011-0078d4?style=for-the-badge&logo=windows)](#prerequisites)

A Windows port of [OneTrueDutchie](https://github.com/pawan0305/onetruedutchie). Same
app, same UI, same features — the macOS Swift audio sidecar (ScreenCaptureKit +
AVAudioEngine) is replaced by an in-process **WASAPI** capture engine, and the
macOS-specific paths/permissions are swapped for their Windows equivalents.

Built originally to translate Dutch standups into English. Now handles any
language Deepgram can recognise, and translates / summarises into any language
Claude (or any OpenAI-compatible model) can write. The name stuck.

- **Transcribe** any meeting (Teams, Zoom, Meet, in-person via mic, podcasts in the browser) in real time
- **Translucent subtitle overlay** that floats above every other window and is fully click-through
- **Translate** every chunk into your target language as it happens
- **Summarise** the whole meeting on demand
- **Chat with the meeting** — ask Claude questions that use the full transcript as context
- **Download transcripts** as .txt — raw (timestamped) or AI-cleaned-and-translated
- **History** with search, tags, and drag-to-merge (combine recordings that should have been one meeting)
- **Custom vocabulary** to boost recognition of names, jargon, and acronyms
- **Audio level VU + cost meter** so you always know it's listening and what you've spent
- 100% local: API keys live in `%APPDATA%\com.onetruedutchie.app\keys.json`, transcripts are JSON files on disk, nothing else leaves your machine besides the calls to Deepgram and your LLM provider.

---

## Quick start

**Install:**

Build the installer (see [Build from source](#build-from-source)), then run
`src-tauri\target\release\bundle\nsis\OneTrueDutchie_0.5.0_x64-setup.exe`.
It installs per-user (no admin prompt) and adds a Start-menu entry.

**Build from source:**

```powershell
git clone <this-repo>
cd onetruedutchie-windows
npm install
npm run tauri build
```

That produces both a standalone `onetruedutchie.exe` and an NSIS installer
under `src-tauri\target\release\bundle\nsis\`.

On first launch:

1. Settings → paste your **Deepgram** and **Anthropic** (or OpenAI-compatible) API keys
2. Pick your **Target language** (default English)
3. Click **Start meeting** — Windows captures system audio (loopback) + your mic automatically
4. Speak, or play a meeting through your speakers/headset, and you're live

> Unlike macOS, Windows needs **no Screen Recording permission** to capture
> system audio — WASAPI loopback works out of the box. See [Permissions](#permissions).

---

## API keys

You need **Deepgram** for transcription. For the LLM (translation, summary,
chat), pick one:

| Component | Provider | Where | Cost per hour |
|-----------|----------|-------|---------------|
| **STT** (required) | Deepgram Nova-3 multi | [console.deepgram.com](https://console.deepgram.com/) — $200 free trial | ≈$0.26 |
| **LLM** option A | Anthropic Claude Haiku 4.5 | [console.anthropic.com](https://console.anthropic.com/) | ≈$0.07 |
| **LLM** option B | OpenAI gpt-4o-mini | [platform.openai.com](https://platform.openai.com/) | ≈$0.05 |
| **LLM** option C | Local model via Ollama / LM Studio / vLLM | localhost | **free** |

Settings → **LLM backend** lets you switch between Anthropic and any
OpenAI-compatible endpoint at any time. Local-model setups have **zero LLM
cost** — your only spend is Deepgram.

Keys are stored in `%APPDATA%\com.onetruedutchie.app\keys.json` and never leave
your machine.

---

## Prerequisites

| Tool | Min version | Notes |
|------|------|------|
| Windows | 10 (1809+) or 11 | |
| WebView2 Runtime | Evergreen | Preinstalled on Windows 11; the installer bundles the bootstrapper otherwise |
| Rust | stable (MSVC toolchain) | `rustup default stable-x86_64-pc-windows-msvc` |
| Visual Studio Build Tools | 2019+ | "Desktop development with C++" workload (MSVC + Windows SDK) |
| Node.js | 20+ | |

---

## Features in detail

### Translucent subtitle overlay
A movie-style subtitle window that's always on top and click-through. Each line
gets a continuous rounded highlight. When unlocked it shows a small floating
control strip with mode toggle (`OFF / source+target / target only`), font size,
lock, and hide buttons — no alt-tab to the main window mid-meeting. Click-through
is implemented with Tauri's `set_ignore_cursor_events`, which sets the
`WS_EX_TRANSPARENT | WS_EX_LAYERED` window styles on Windows.

### Multi-language
Source language defaults to auto-detect (`language=multi` on Nova-3), but
Settings → **Source language** lets you lock to a single language (Dutch,
Flemish, English, German, …) which is noticeably more accurate than auto-detect
when you know what's being spoken. Target language for translation, summary, and
chat is set separately — pick from 20 common options or type any language the LLM
knows. Default English.

### Drag-to-merge history
Grab the `⋮⋮` handle next to one history row and drop it onto another. Segments,
chat, notes, and tags from both recordings are combined and re-sorted by
timestamp — the merged transcript reads chronologically.

### Transcript downloads
Two buttons in the transcript pane: **↓ Raw .txt** writes the verbatim
transcript with `[HH:MM:SS]` timestamps to your Downloads folder, instantly.
**↓ Cleaned .txt** runs the transcript through your LLM to fix mistranscriptions
and translate it into your target language, preserving the timestamp structure.
Long transcripts are chunked so nothing truncates.

### Custom vocabulary
Settings → Custom vocabulary. One term per line. Boosts these via Deepgram
Nova-3 `keyterm=` — useful for colleague names, project codenames, and jargon.

### Auto-reconnect
A `tokio::sync::broadcast` channel fans out the live audio so if the Deepgram
WebSocket drops mid-meeting, we transparently reconnect (with exponential
backoff) and the audio engine keeps running. A coloured dot in the top bar shows
connection state.

### Cost & audio level meters
Top bar shows running per-meeting cost (Deepgram seconds + LLM tokens) and two VU
bars (mic + system audio) so you can see at a glance that audio is flowing.

### Notes pane + collapsible sections
Each meeting has a freeform notes textarea (debounced autosave). Any pane
(Transcript / Summary / Chat / Notes) can be collapsed to a vertical strip.

---

## Architecture

```
                ┌─────────────────────────────────┐
                │   Tauri main window (React)      │
                │   TopBar │ Transcript │ Summary  │
                │   Chat   │ Notes      │ History  │
                └──────────────────┬───────────────┘
                                   │ Tauri events / invoke
                                   ▼
   ┌──────────────────────────────────────────────────┐
   │  Tauri overlay window (React, transparent)       │
   │  Subtitles + controls when unlocked              │
   └──────────────────────────────────────────────────┘
                                   ▲
                                   │
   ┌──────────────────────────────────────────────────┐
   │  Rust core (src-tauri/src/)                      │
   │   commands.rs   ← meeting orchestrator           │
   │   audio.rs      ← WASAPI capture + mixer (NEW)   │
   │   deepgram.rs   ← live STT WebSocket             │
   │   anthropic.rs / openai.rs ← translate/summary   │
   │   settings.rs   ← keys.json on disk              │
   │   storage.rs    ← per-meeting JSON files         │
   └──────────────────────┬───────────────────────────┘
                          │ 16 kHz mono Int16 PCM via mpsc channel
                          ▼
   ┌─────────────────────────────────────────────────┐
   │  WASAPI capture (in-process, audio.rs)           │
   │   default render endpoint  → loopback (system)   │
   │   default capture endpoint → microphone          │
   │   → mono → linear-resample to 16 kHz → mix        │
   └─────────────────────────────────────────────────┘
```

**Flow for one utterance:**

1. Two capture threads read WASAPI buffers — one on the default **render**
   endpoint with the `AUDCLNT_STREAMFLAGS_LOOPBACK` flag (everything other apps
   are playing), one on the default **capture** endpoint (your mic). Each frame
   is averaged to mono and resampled to 16 kHz.
2. A mixer thread drains both rings every 100 ms, sums them sample-aligned
   (clamped to Int16), and forwards a single 16 kHz mono Int16 LE stream over an
   mpsc channel. It also emits `audio:level` events for the VU meter.
3. Rust broadcasts that stream so a fresh Deepgram session can resubscribe after
   a disconnect without losing audio.
4. The Deepgram WebSocket consumer streams to Nova-3 with `language=multi`,
   `keyterm=<your vocab>`. Interim text shows up as `segment:pending` events.
5. On `is_final` the segment is committed — a background task calls the LLM to
   translate the chunk into your target language.
6. Summary / chat calls send the running transcript with prompt caching so
   follow-up calls are cheap.

Mixing both sources *sample-by-sample* (rather than concatenating whichever
arrived first) is what prevents "everything appears twice in the transcript"
when mic sidetone bleeds into the system loopback.

---

## Manual dev workflow

```powershell
# JS deps
npm install

# Dev mode (hot-reload frontend, real Rust backend)
npm run tauri dev
```

To build a distributable installer instead:

```powershell
npm run tauri build
```

Verify the system-audio loopback path on your machine (play something through
your speakers, then run):

```powershell
cargo run --example loopback_probe --manifest-path src-tauri/Cargo.toml
# → prints the mix format and "LOOPBACK_OK" with a non-zero sample count
```

---

## Permissions

| Permission | Why |
|-----------|-----|
| **Microphone** | Optional mic capture (your own voice). Windows Settings → Privacy & security → Microphone must allow desktop apps. |
| **System audio** | None required — WASAPI loopback on the render endpoint captures other apps' audio with no special permission. |

If the mic doesn't capture, check **Settings → Privacy & security → Microphone →
Let desktop apps access your microphone**. The meeting still runs on system audio
alone if the mic is unavailable.

---

## Troubleshooting

**No transcript appears after starting**
Make sure your Deepgram key is set and has credit (Settings). Confirm audio is
actually playing — the top-bar VU bars should move. Run the `loopback_probe`
example (above) to confirm system audio is being captured.

**Transcript appears but no translation**
Your LLM key is wrong, has no credit, or `translate` is toggled off (top bar 🌐
button). Re-check Settings and the toggle.

**The window opens but is blank / "can't reach this page"**
That happens only with a raw `cargo build` debug binary (it points at the Vite
dev server). Use `npm run tauri dev` for development, or run the bundled
release build, which serves the frontend from embedded assets.

**Subtitle overlay is opaque**
Transparent windows need the WebView2 Runtime. It's preinstalled on Windows 11;
on Windows 10 the NSIS installer downloads the bootstrapper.

**Logs**
The app writes to `%LOCALAPPDATA%\com.onetruedutchie.app\logs\onetrue.log`
(the release build has no attached console).

---

## Project layout

```
onetruedutchie-windows/
├── src/                         React + TypeScript frontend (shared with macOS)
│   ├── App.tsx                  Root + Tauri event subscriptions
│   ├── components/              TopBar, Transcript, Summary, Chat, Notes, Settings, History
│   └── overlay/                 Subtitle overlay + control strip
├── src-tauri/
│   ├── src/
│   │   ├── lib.rs               Tauri builder (Windows log path)
│   │   ├── commands.rs          IPC commands + meeting orchestrator
│   │   ├── audio.rs             WASAPI loopback + mic capture, resample, mix
│   │   ├── deepgram.rs          Deepgram WebSocket client
│   │   ├── anthropic.rs         Claude translate/summary/chat
│   │   ├── openai.rs            OpenAI-compatible backend
│   │   ├── llm.rs               Backend dispatcher
│   │   ├── settings.rs          keys.json on disk (%APPDATA%)
│   │   ├── storage.rs           Per-meeting JSON file persistence
│   │   └── state.rs             Meeting / Segment / Chat / Cost model
│   ├── examples/
│   │   └── loopback_probe.rs    Diagnostic: verify WASAPI loopback works
│   ├── tauri.conf.json          Two windows: main + overlay; NSIS bundle
│   └── capabilities/default.json
└── scripts/
    └── build.ps1                One-shot build helper
```

### What changed from the macOS original

- **`audio.rs`** rewritten: macOS Swift sidecar → in-process WASAPI (the
  `windows` crate). Same 16 kHz mono Int16 LE output contract, so the rest of
  the pipeline is unchanged.
- **Paths**: `~/Library/Application Support` → `%APPDATA%`, `~/Library/Logs` →
  `%LOCALAPPDATA%`, `~/Downloads` → `%USERPROFILE%\Downloads`.
- **Bundle**: `dmg`/`app` → `nsis` installer; dropped `macOSPrivateApi`,
  `externalBin`, and `Info.plist`.
- **Config robustness**: `keys.json` reading now tolerates a UTF-8 BOM (Notepad
  / PowerShell add one).
- Everything else — the React frontend, the orchestrator, Deepgram, the LLM
  backends, storage, history/merge — is the original, unchanged.

---

## License

MIT — see [LICENSE](LICENSE).

The name "OneTrueDutchie" is just a name — the project translates from and to
any language, not just Dutch.
