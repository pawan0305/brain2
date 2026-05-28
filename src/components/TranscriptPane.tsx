import { useEffect, useRef, useState } from "react";
import { api } from "../lib/tauri";
import type { Segment } from "../lib/types";

interface Props {
  segments: Segment[];
  pendingId?: string;
  meetingId?: string;
  showEnglish?: boolean;
  onError?: (msg: string) => void;
  onCollapse?: () => void;
}

function fmtTime(iso: string): string {
  const d = new Date(iso);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  return `${hh}:${mm}:${ss}`;
}

/** Source-language transcript: one line per finalised segment, no timestamps. */
function buildDutchTranscript(segments: Segment[]): string {
  return segments
    .filter((s) => s.is_final)
    .map((s) => s.dutch.trim())
    .filter((line) => line.length > 0)
    .join("\n");
}

/** English transcript built from the per-chunk translations that have
 *  already been computed live. Falls back to the source text for segments
 *  where translation was disabled (or where the segment was already
 *  English so `english === dutch`). Instant — no Claude round-trip. */
function buildEnglishTranscript(segments: Segment[]): string {
  return segments
    .filter((s) => s.is_final)
    .map((s) => (s.english ?? s.dutch).trim())
    .filter((line) => line.length > 0)
    .join("\n");
}

type CopyKind = "nl" | "en";
type DownloadKind = "raw" | "cleaned";

export function TranscriptPane({
  segments,
  pendingId,
  meetingId,
  showEnglish = true,
  onError,
  onCollapse,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottomRef = useRef(true);
  const [copied, setCopied] = useState<CopyKind | null>(null);
  const [downloading, setDownloading] = useState<DownloadKind | null>(null);
  const [downloadedPath, setDownloadedPath] = useState<string | null>(null);

  const doDownload = async (kind: DownloadKind) => {
    if (downloading) return;
    setDownloading(kind);
    setDownloadedPath(null);
    try {
      const path =
        kind === "raw"
          ? await api.exportRawTranscriptFile(meetingId)
          : await api.exportCleanedTranslatedTranscriptFile(meetingId);
      // Show "✓ saved to ~/Downloads/…" briefly. Trim the home prefix so
      // the toast stays readable.
      const display = path.replace(/^\/Users\/[^/]+/, "~");
      setDownloadedPath(display);
      setTimeout(() => setDownloadedPath((p) => (p === display ? null : p)), 6000);
    } catch (err) {
      onError?.(`download ${kind}: ${err}`);
    } finally {
      setDownloading(null);
    }
  };

  const doCopy = async (kind: CopyKind) => {
    const text =
      kind === "nl"
        ? buildDutchTranscript(segments)
        : buildEnglishTranscript(segments);
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(kind);
      setTimeout(() => setCopied((c) => (c === kind ? null : c)), 1500);
    } catch {
      /* clipboard blocked */
    }
  };

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (stickToBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [segments.length, segments.at(-1)?.dutch, segments.at(-1)?.english]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 32;
    stickToBottomRef.current = atBottom;
  };

  const hasFinal = segments.some((s) => s.is_final);

  return (
    <section className="pane transcript-pane">
      <header className="pane-header">
        <h2>Transcript</h2>
        <div className="pane-sub-row">
          <button
            className="ghost"
            onClick={() => doCopy("nl")}
            disabled={!hasFinal}
            title="Copy the raw source-language transcript"
          >
            {copied === "nl" ? "✓ copied" : "Copy NL"}
          </button>
          <button
            className="ghost"
            onClick={() => doCopy("en")}
            disabled={!hasFinal}
            title="Copy the live English transcript (per-chunk translations, instant)"
          >
            {copied === "en" ? "✓ copied" : "Copy EN"}
          </button>
          <button
            className="ghost"
            onClick={() => doDownload("raw")}
            disabled={!hasFinal || downloading !== null}
            title="Download raw transcript as .txt (timestamps + speakers) to ~/Downloads"
          >
            {downloading === "raw" ? "Saving…" : "↓ Raw .txt"}
          </button>
          <button
            className="ghost"
            onClick={() => doDownload("cleaned")}
            disabled={!hasFinal || downloading !== null}
            title="LLM cleans up misheard words / jargon / metaphors, then translates. Saves to ~/Downloads."
          >
            {downloading === "cleaned" ? "Cleaning + translating…" : "↓ Cleaned .txt"}
          </button>
          <span className="pane-sub">{segments.length} segments</span>
          {onCollapse && (
            <button className="ghost" onClick={onCollapse} title="Collapse">
              ◀
            </button>
          )}
        </div>
        {downloadedPath && (
          <div className="download-toast">
            ✓ saved to <code>{downloadedPath}</code>
          </div>
        )}
      </header>
      <div className="pane-body scroll" ref={scrollRef} onScroll={onScroll}>
        {segments.length === 0 && (
          <div className="empty">
            Press <strong>Start meeting</strong> to begin live transcription.
          </div>
        )}
        {segments.map((s) => (
          <div
            key={s.id}
            className={`segment${s.id === pendingId ? " pending" : ""}${
              s.is_final ? " final" : ""
            }`}
          >
            <div className="segment-time">{fmtTime(s.started_at)}</div>
            <div className={`segment-cols${showEnglish ? "" : " single"}`}>
              <div className="col nl">
                {showEnglish && <div className="lang-label">NL</div>}
                <div className="text">{s.dutch || <em>…</em>}</div>
              </div>
              {showEnglish && (
                <div className="col en">
                  <div className="lang-label">EN</div>
                  <div className="text">
                    {s.english ?? (s.is_final ? <em className="muted">translating…</em> : <em className="muted">—</em>)}
                  </div>
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </section>
  );
}
