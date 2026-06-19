import { useEffect, useState } from "react";
import type { AudioLevel, DgStatus, Meeting, MeetingCost, SettingsView } from "../lib/types";

interface Props {
  meeting: Meeting | null;
  running: boolean;
  paused: boolean;
  settings: SettingsView | null;
  audioLevel: AudioLevel;
  dgStatus: DgStatus;
  cost: MeetingCost | null;
  stack?: Record<string, { state: "ok" | "starting" | "down"; detail: string }>;
  agentStatus?: { state: "warming" | "ready" | "error"; error?: string } | null;
  feedNote?: string | null;
  onStart: () => void;
  onStop: () => void;
  onTogglePause: () => void;
  onOpenSettings: () => void;
  onOpenHistory: () => void;
  onRenameMeeting: (title: string) => void;
  onToggleTranslate: (enabled: boolean) => void;
  onCycleOverlay: () => void;
  onChangeOverlayFontSize: (delta: number) => void;
  onToggleOverlayLocked: () => void;
  onToggleFeed?: () => void;
}

/// Deepgram nova-3 streaming ≈ $0.0043 / minute. Haiku 4.5 ≈ $1/Mtok in,
/// $5/Mtok out, $0.1/Mtok cache-read. Rough $ ballpark for the top bar.
function estimateCost(c: MeetingCost): number {
  const dg = (c.deepgram_audio_secs / 60) * 0.0043;
  const an =
    (c.anthropic_input_tokens / 1_000_000) * 1.0 +
    (c.anthropic_output_tokens / 1_000_000) * 5.0 +
    (c.anthropic_cache_read_tokens / 1_000_000) * 0.1;
  return dg + an;
}

/** A single dot+label in the stack health strip. */
function StackPill({
  label,
  state,
  detail,
}: {
  label: string;
  state: "ok" | "starting" | "down" | "idle";
  detail: string;
}) {
  return (
    <span className={`stack-pill ${state}`} title={`${label}: ${detail}`}>
      <span className="dot" />
      {label}
    </span>
  );
}

/** The cockpit health strip: WSL · Ollama · gbrain · Claude. */
function StackStrip({
  stack,
  agentStatus,
}: {
  stack: Record<string, { state: "ok" | "starting" | "down"; detail: string }>;
  agentStatus?: { state: "warming" | "ready" | "error"; error?: string } | null;
}) {
  const claude: "ok" | "starting" | "down" | "idle" =
    agentStatus?.state === "ready"
      ? "ok"
      : agentStatus?.state === "warming"
        ? "starting"
        : agentStatus?.state === "error"
          ? "down"
          : "idle";
  const claudeDetail =
    agentStatus?.state === "error"
      ? (agentStatus.error ?? "warm-up failed")
      : agentStatus?.state ?? "not started (Direct backend)";
  const cell = (key: string): { state: "ok" | "starting" | "down" | "idle"; detail: string } =>
    stack[key] ? { state: stack[key].state, detail: stack[key].detail } : { state: "idle", detail: "checking…" };
  const wsl = cell("wsl");
  const ollama = cell("ollama");
  const gbrain = cell("gbrain");
  return (
    <div className="stack-strip" title="Local 2nd-brain stack">
      <StackPill label="WSL" state={wsl.state} detail={wsl.detail} />
      <StackPill label="Ollama" state={ollama.state} detail={ollama.detail} />
      <StackPill label="gbrain" state={gbrain.state} detail={gbrain.detail} />
      <StackPill label="Claude" state={claude} detail={claudeDetail} />
    </div>
  );
}

function VuBar({ value, color }: { value: number; color: string }) {
  // Compress to log-ish scale so quiet speech is visible.
  const v = Math.max(0, Math.min(1, value));
  const pct = Math.min(100, Math.round(Math.sqrt(v) * 140));
  return (
    <div className="vu-track">
      <div className="vu-fill" style={{ width: `${pct}%`, background: color }} />
    </div>
  );
}

export function TopBar({
  meeting,
  running,
  paused,
  settings,
  audioLevel,
  dgStatus,
  cost,
  stack = {},
  agentStatus,
  feedNote,
  onStart,
  onStop,
  onTogglePause,
  onOpenSettings,
  onOpenHistory,
  onRenameMeeting,
  onToggleTranslate,
  onCycleOverlay,
  onChangeOverlayFontSize,
  onToggleOverlayLocked,
  onToggleFeed,
}: Props) {
  const [editingTitle, setEditingTitle] = useState<string | null>(null);

  useEffect(() => {
    if (!running) setEditingTitle(null);
  }, [running, meeting?.id]);

  const keysOk = !!settings?.deepgram_set && !!settings?.anthropic_set;

  return (
    <header className="topbar">
      <div className="topbar-left">
        <span className="brand">Brain2</span>
        <StackStrip stack={stack} agentStatus={agentStatus} />
        {settings && (
          <button
            className={`ghost feed-toggle ${settings.brain_feed_enabled ? "on" : "muted"}`}
            onClick={onToggleFeed}
            title={
              settings.brain_feed_enabled
                ? "Brain feed ON — finished meetings + project work are distilled into your gbrain. Click to pause."
                : "Brain feed OFF — click to resume populating your 2nd brain."
            }
          >
            🧠 Feed: {settings.brain_feed_enabled ? "on" : "off"}
          </button>
        )}
        {feedNote && <span className="pane-sub feed-note">{feedNote}</span>}
        {meeting ? (
          editingTitle !== null ? (
            <input
              className="title-input"
              value={editingTitle}
              autoFocus
              onChange={(e) => setEditingTitle(e.target.value)}
              onBlur={() => {
                if (editingTitle.trim()) onRenameMeeting(editingTitle.trim());
                setEditingTitle(null);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
                if (e.key === "Escape") setEditingTitle(null);
              }}
            />
          ) : (
            <span
              className="title"
              onClick={() => running && setEditingTitle(meeting.title)}
              title={running ? "click to rename" : ""}
            >
              {meeting.title}
              {running && !paused && <span className="rec-dot" />}
              {running && paused && (
                <span className="paused-badge" title="Meeting is paused">
                  ⏸ paused
                </span>
              )}
            </span>
          )
        ) : (
          <span className="title muted">No meeting</span>
        )}
      </div>
      <div className="topbar-right">
        {running && (
          <div className="vu-meters" title={`Mic ${(audioLevel.mic * 100).toFixed(0)}% / Sys ${(audioLevel.sys * 100).toFixed(0)}%`}>
            <div className="vu-row">
              <span className="vu-label">M</span>
              <VuBar value={audioLevel.mic} color="#ffb380" />
            </div>
            <div className="vu-row">
              <span className="vu-label">S</span>
              <VuBar value={audioLevel.sys} color="#7fb3ff" />
            </div>
          </div>
        )}
        {running && (
          <span
            className={`dg-status dg-status-${dgStatus}`}
            title={
              dgStatus === "connected" ? "Deepgram connected"
              : dgStatus === "reconnecting" ? "Deepgram reconnecting…"
              : "Deepgram disconnected"
            }
          />
        )}
        {cost && (cost.deepgram_audio_secs > 0 || cost.anthropic_output_tokens > 0) && (
          <span
            className="pane-sub"
            title={`Deepgram ${(cost.deepgram_audio_secs / 60).toFixed(1)} min · Claude in ${cost.anthropic_input_tokens} / out ${cost.anthropic_output_tokens}`}
          >
            ${estimateCost(cost).toFixed(3)}
          </span>
        )}
        {!keysOk && (
          <span className="warn" onClick={onOpenSettings}>
            ⚠ keys not set
          </span>
        )}
        <button
          className={`ghost ${settings?.translate ? "" : "muted"}`}
          onClick={() => onToggleTranslate(!(settings?.translate ?? true))}
          title={settings?.translate
            ? "Translation on — click to turn off"
            : "Translation off — click to turn on"}
        >
          Translate: {settings?.translate ? "on" : "off"}
        </button>
        <button
          className={`ghost ${settings?.overlay_mode === "off" ? "muted" : ""}`}
          onClick={onCycleOverlay}
          title="Subtitles overlay: click to cycle off → dual → EN-only"
        >
          Subtitles: {
            settings?.overlay_mode === "dual" ? "NL+EN"
              : settings?.overlay_mode === "en" ? "EN"
              : "off"
          }
        </button>
        {settings?.overlay_mode !== "off" && (
          <>
            <button
              className="ghost"
              onClick={() => onChangeOverlayFontSize(-2)}
              title="Smaller subtitle text"
            >
              A−
            </button>
            <span className="pane-sub" style={{ minWidth: 22, textAlign: "center" }}>
              {settings?.overlay_font_size ?? 24}
            </span>
            <button
              className="ghost"
              onClick={() => onChangeOverlayFontSize(2)}
              title="Bigger subtitle text"
            >
              A+
            </button>
            <button
              className="ghost"
              onClick={onToggleOverlayLocked}
              title={settings?.overlay_locked
                ? "Locked (click-through). Click to unlock and drag/resize."
                : "Unlocked. Click to lock and make click-through."}
            >
              {settings?.overlay_locked ? "🔒" : "🔓"}
            </button>
          </>
        )}
        <button onClick={onOpenHistory}>History</button>
        {running ? (
          <>
            <button
              className={paused ? "primary" : "ghost"}
              onClick={onTogglePause}
              title={
                paused
                  ? "Resume — bytes start flowing to Deepgram again"
                  : "Pause — stop billing for Deepgram + Claude during a break"
              }
            >
              {paused ? "▶ Resume" : "⏸ Pause"}
            </button>
            <button className="primary danger" onClick={onStop}>
              ◼ Stop
            </button>
          </>
        ) : (
          <button
            className="primary"
            onClick={onStart}
            disabled={!keysOk}
            title={keysOk ? "" : "set API keys first"}
          >
            ● Start meeting
          </button>
        )}
        <button onClick={onOpenSettings} title="Settings">
          ⚙
        </button>
      </div>
    </header>
  );
}
