# 🧠 Brain2

**Your 2nd brain during conversations.** Live transcription, proactive AI co-pilot, self-improving desktop app.

Brain2 captures your meetings (Teams, Zoom, browser), transcribes in real time via Deepgram, translates via Claude/OpenAI/Ollama, and now includes:

- **⚒️ Forge** — a self-modifying agent that can improve its own source code, rebuild itself, and self-update
- **🧠 Brain Engine** — detects action items, logs decisions, recalls context from past meetings, generates wrap-ups
- **🏭 AI Factory Connector** — reports metrics to the AI Factory, receives factory-built updates

Originally built as "OneTrueDutchie" — a Dutch→English meeting translator. Now Brain2 works with any language Deepgram can recognize.

## Quick Start

Download the latest portable `.exe` from [Releases](https://github.com/pawan0305/brain2/releases).

Or build from source:

```powershell
git clone https://github.com/pawan0305/brain2.git
cd brain2
npm install
npm run tauri build
```

The built exe will be in `src-tauri\target\release\bundle\nsis\`.

You'll need:
- **Deepgram API key** (for transcription) — [console.deepgram.com](https://console.deepgram.com)
- **Anthropic API key** (for translation, summary, chat, Forge, Brain) — or use Ollama locally
- **Node.js 22+** and **Rust** (for building from source)

## Features

| Tab | What it does |
|-----|-------------|
| **Transcript** | Live transcription + English translation with speaker labels |
| **Summary** | AI-generated meeting summary, regeneratable |
| **Ask the meeting** | Chat with the transcript — ask questions, get answers |
| **Notes** | Freeform notes synced with the meeting |
| **Forge** | Self-improvement agent — modify Brain2's own source code |

### Forge — self-improvement

The Forge tab lets Brain2 improve itself:

1. **Init** — clones the Brain2 repo from GitHub into a local workspace
2. **Chat** — describe what you want ("add dark mode", "improve speaker detection")
3. **Review** — the agent proposes changes, you preview the diff
4. **Approve/Reject** — human gate at every step
5. **Build** — compiles a new portable exe
6. **Install** — replaces the current exe, keeps backup for rollback

Every change is git-tracked and pushed to GitHub. Heavy architectural changes are routed to the AI Factory's Cerberus crew.

### Brain Engine — proactive co-pilot

While you're in a meeting, Brain2's engine works in the background:

- **Detects action items** — "I'll send that by Friday" → logged as a task
- **Logs decisions** — "So we agree to use Rust" → recorded with context
- **Recalls context** — "We discussed this with Martijn last Tuesday" → surfaces relevant history
- **Generates wrap-ups** — consolidates actions, decisions, and next steps

### AI Factory Connector

Brain2 reports back to the AI Factory:

- Usage metrics (meeting count, audio hours, costs)
- Error patterns for analysis
- Improvement ideas for the Cerberus crew
- Checks for factory-built updates

## Architecture

```
Brain2 (Tauri 2, portable .exe)
├── Deepgram WebSocket    — real-time STT
├── Anthropic / OpenAI    — translation, summary, chat, Forge, Brain
│   └── Ollama fallback   — local LLM option (free, offline)
├── Rust backend           — commands, audio, state, storage, settings
├── React frontend         — TranscriptPane, SummaryPane, ChatPane,
│                            NotesPane, ForgePane, SettingsModal
├── Forge engine           — self-modifying agent (Git + Claude + cargo)
├── Brain engine           — action detection, decision logging, memory
└── Factory connector      — metrics, updates, idea pipeline
```

## Configuration

All configuration is in-app via **Settings** (gear icon) and is stored in
`%APPDATA%\com.brain2.app\keys.json`:

- **API keys** — Deepgram (cloud STT) and Anthropic (agent/LLM).
- **STT engine** — Deepgram (cloud) or local Whisper (on-device, Vulkan).
- **LLM provider** — Anthropic, or any OpenAI-compatible endpoint (Ollama, etc.).
- **Brain2 agent backend** — Direct, Claude Code, or Hermes.
- **Languages** — source + target, custom vocabulary, subtitles overlay.

See [RELEASES.md](RELEASES.md) for prebuilt downloads.

## License

MIT — see [LICENSE](LICENSE).
