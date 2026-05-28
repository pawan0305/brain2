import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

interface Segment {
  id: string;
  dutch: string;
  english: string | null;
  is_final: boolean;
}

interface Settings {
  overlay_mode: string;
  overlay_font_size: number;
  overlay_locked: boolean;
  target_language: string;
}

const MAX_LINES = 3;

// Human-language-name → short uppercase code for the overlay button.
// Falls back to first two letters of the name uppercased for anything
// not in this table.
const LANG_CODE: Record<string, string> = {
  English: "EN",
  Dutch: "NL",
  Spanish: "ES",
  French: "FR",
  German: "DE",
  Italian: "IT",
  Portuguese: "PT",
  Polish: "PL",
  Russian: "RU",
  Ukrainian: "UA",
  Turkish: "TR",
  Arabic: "AR",
  Hindi: "HI",
  "Chinese (Simplified)": "ZH",
  "Chinese (Traditional)": "ZH",
  Japanese: "JA",
  Korean: "KO",
  Indonesian: "ID",
  Vietnamese: "VI",
  Thai: "TH",
};
const langCode = (name: string): string =>
  LANG_CODE[name] || (name || "EN").replace(/[^A-Za-z]/g, "").slice(0, 2).toUpperCase() || "EN";

export function Overlay() {
  const [segments, setSegments] = useState<Segment[]>([]);
  const [pending, setPending] = useState<Segment | null>(null);
  const [mode, setMode] = useState<string>("dual");
  const [fontSize, setFontSize] = useState<number>(24);
  const [locked, setLocked] = useState<boolean>(true);
  const [targetLang, setTargetLang] = useState<string>("English");

  useEffect(() => {
    invoke<Settings>("get_settings")
      .then((s) => {
        setMode(s.overlay_mode || "dual");
        if (s.overlay_font_size) setFontSize(s.overlay_font_size);
        if (typeof s.overlay_locked === "boolean") setLocked(s.overlay_locked);
        if (s.target_language) setTargetLang(s.target_language);
      })
      .catch(() => {});

    const promises: Promise<UnlistenFn>[] = [
      listen<Segment>("segment:upsert", (e) => {
        const seg = e.payload;
        setPending(null);
        setSegments((prev) => {
          // Replace if same id (translation update), else append.
          const idx = prev.findIndex((s) => s.id === seg.id);
          let next = idx >= 0
            ? prev.map((s, i) => (i === idx ? seg : s))
            : [...prev, seg];
          if (next.length > MAX_LINES) next = next.slice(next.length - MAX_LINES);
          return next;
        });
      }),
      listen<{ id: string; english: string | null }>("segment:translated", (e) => {
        setSegments((prev) =>
          prev.map((s) =>
            s.id === e.payload.id ? { ...s, english: e.payload.english } : s,
          ),
        );
      }),
      listen<Segment>("segment:pending", (e) => setPending(e.payload)),
      listen<{ mode: string }>("overlay:mode", (e) => setMode(e.payload.mode)),
      listen<{ font_size: number; locked: boolean }>("overlay:settings", (e) => {
        if (e.payload.font_size) setFontSize(e.payload.font_size);
        if (typeof e.payload.locked === "boolean") setLocked(e.payload.locked);
      }),
      listen<{ target_language: string }>("overlay:target_language", (e) => {
        if (e.payload.target_language) setTargetLang(e.payload.target_language);
      }),
      listen<unknown>("meeting:started", () => {
        // Fresh meeting → clear stale subtitles.
        setSegments([]);
        setPending(null);
      }),
    ];
    let off: UnlistenFn[] = [];
    Promise.all(promises).then((arr) => {
      off = arr;
    });
    return () => {
      off.forEach((fn) => fn());
    };
  }, []);

  // Persist overlay window position + size whenever the user drags or
  // resizes it. Debounced so we don't write keys.json 60×/sec.
  const saveTimer = useRef<number | null>(null);
  useEffect(() => {
    const win = getCurrentWebviewWindow();
    const persist = async () => {
      try {
        const pos = await win.outerPosition();
        const size = await win.outerSize();
        await invoke("save_overlay_geometry", {
          x: pos.x,
          y: pos.y,
          w: size.width,
          h: size.height,
        });
      } catch {
        /* ignore */
      }
    };
    const schedule = () => {
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(persist, 400);
    };
    const off: UnlistenFn[] = [];
    win.onMoved(schedule).then((u) => off.push(u));
    win.onResized(schedule).then((u) => off.push(u));
    return () => {
      off.forEach((fn) => fn());
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
    };
  }, []);

  const showNL = mode === "dual";
  const showEN = mode === "dual" || mode === "en";

  const cycleMode = async () => {
    const next = mode === "off" ? "dual" : mode === "dual" ? "en" : "off";
    setMode(next);
    try {
      await invoke("set_overlay_mode", { mode: next });
    } catch {
      /* ignore */
    }
  };
  const bumpFont = async (delta: number) => {
    const next = Math.max(12, Math.min(72, fontSize + delta));
    setFontSize(next);
    try {
      await invoke("set_overlay_font_size", { size: next });
    } catch {
      /* ignore */
    }
  };
  const lockOverlay = async () => {
    setLocked(true);
    try {
      await invoke("set_overlay_locked", { locked: true });
    } catch {
      /* ignore */
    }
  };
  const hideOverlay = async () => {
    setMode("off");
    try {
      await invoke("set_overlay_mode", { mode: "off" });
    } catch {
      /* ignore */
    }
  };
  // Show "OFF / AUTO+XX / XX" where XX is the target-language code. AUTO = the
  // auto-detected source language Deepgram is picking up live.
  const tCode = langCode(targetLang);
  const modeLabel =
    mode === "off" ? "OFF" : mode === "dual" ? `AUTO+${tCode}` : tCode;

  return (
    <div
      className={`overlay-shell${locked ? "" : " unlocked"}`}
      data-tauri-drag-region
      style={{ ["--overlay-font-size" as any]: `${fontSize}px` }}
    >
      {!locked && (
        <div className="overlay-controls" onMouseDown={(e) => e.stopPropagation()}>
          <button
            className="ovc-btn"
            title="Cycle subtitles: off / source+target / target only"
            onClick={cycleMode}
          >
            🌐 {modeLabel}
          </button>
          <button className="ovc-btn" title="Smaller text" onClick={() => bumpFont(-2)}>
            A−
          </button>
          <button className="ovc-btn" title="Larger text" onClick={() => bumpFont(+2)}>
            A+
          </button>
          <button
            className="ovc-btn"
            title="Lock (click-through). Unlock from the main window."
            onClick={lockOverlay}
          >
            🔒
          </button>
          <button
            className="ovc-btn"
            title="Hide subtitles (sets mode to off)"
            onClick={hideOverlay}
          >
            ✕
          </button>
        </div>
      )}
      <div className="overlay-lines" data-tauri-drag-region>
        {segments.length === 0 && !pending && (
          <div className="overlay-line muted" data-tauri-drag-region>
            <div className="line"><span className="hl muted">…waiting for speech…</span></div>
          </div>
        )}
        {segments.map((s) => (
          <div key={s.id} className="overlay-line" data-tauri-drag-region>
            {showNL && (
              <div className="line"><span className="hl nl">{s.dutch}</span></div>
            )}
            {showEN && (
              <div className="line">
                <span className="hl en">
                  {s.english ?? <span className="muted">translating…</span>}
                </span>
              </div>
            )}
          </div>
        ))}
        {pending && (
          <div className="overlay-line pending" data-tauri-drag-region>
            {showNL && (
              <div className="line"><span className="hl nl">{pending.dutch}</span></div>
            )}
          </div>
        )}
      </div>
      <div
        className="resize-handle"
        title="drag to resize"
        onMouseDown={(e) => {
          e.preventDefault();
          e.stopPropagation();
          // Tauri 2: ResizeDirection is a string; "SouthEast" expands from BR.
          getCurrentWebviewWindow().startResizeDragging("SouthEast" as any);
        }}
      />
    </div>
  );
}
