# Brain2 — Downloads

Prebuilt Windows binaries are published on the **[Releases page](https://github.com/pawan0305/brain2/releases)** (kept out of the git tree so they don't bloat history).

## Latest — v0.6.3

| Download | What it is |
|---|---|
| [**Brain2-0.6.3-portable-x64.exe**](https://github.com/pawan0305/brain2/releases/download/v0.6.3/Brain2-0.6.3-portable-x64.exe) | **Portable single .exe** — just run it, no install (~70 MB; bundles on-device Whisper STT). |
| [Brain2_0.6.3_x64-setup.exe](https://github.com/pawan0305/brain2/releases/download/v0.6.3/Brain2_0.6.3_x64-setup.exe) | NSIS installer (~9 MB). |

v0.6.3 is a production-readiness pass: no console-window flashing, better local transcription, Forge removed, fixed settings storage, Windows-native fonts. See the [release notes](https://github.com/pawan0305/brain2/releases/tag/v0.6.3).

This build includes **on-device, Vulkan-accelerated Whisper STT**, so meeting audio can be transcribed locally without leaving the machine.

### Requirements
- Windows 11 x64 (WebView2 runtime is preinstalled on Win11).
- A GPU with Vulkan drivers for local Whisper STT (Intel Arc / NVIDIA / …).
- The agentic features — gbrain-backed *Ask the meeting*, the brain feeder, and the gbrain MCP server — need the local stack (**WSL + gbrain + Ollama**). Without it, core transcription + translation still work.

### Building it yourself
- `scripts/build-portable.bat` — release (optimized) build with local Whisper.
- `scripts/run-local-stt.bat` — debug build with local Whisper.
- Toolchain (no-admin): `scoop install llvm ninja` + the Vulkan SDK; see `scripts/build-local-stt.bat`.
