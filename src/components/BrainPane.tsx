import { useCallback, useEffect, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../lib/tauri";
import type { BrainStatus, Meeting } from "../lib/types";

// The Brain pane surfaces what the Brain engine detects live during a meeting:
// action items, decisions, a running event feed, and an on-demand wrap-up. The
// backend emits `brain:status` on every update; we also load once on mount.
export function BrainPane({
  meeting,
  onError,
  onCollapse,
}: {
  meeting: Meeting | null;
  onError: (msg: string) => void;
  onCollapse: () => void;
}) {
  const [status, setStatus] = useState<BrainStatus | null>(null);
  const [wrapUp, setWrapUp] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await api.brainStatus());
    } catch (err) {
      onError(`brain status: ${err}`);
    }
  }, [onError]);

  useEffect(() => {
    refresh();
    const un = listen<BrainStatus>("brain:status", (e) => setStatus(e.payload));
    return () => {
      un.then((fn) => fn()).catch(() => {});
    };
  }, [refresh]);

  const toggle = async (enabled: boolean) => {
    try {
      setStatus(await api.brainToggle(enabled));
    } catch (err) {
      onError(`brain toggle: ${err}`);
    }
  };

  const markDone = async (id: string) => {
    try {
      setStatus(await api.brainMarkActionDone(id));
    } catch (err) {
      onError(`mark done: ${err}`);
    }
  };

  const doWrapUp = async () => {
    if (!meeting) return;
    setBusy(true);
    try {
      const transcript = meeting.segments
        .filter((s) => s.is_final)
        .map((s) => s.dutch.trim())
        .filter(Boolean)
        .join("\n");
      const text = await api.brainWrapUp(meeting.id, meeting.title, transcript);
      setWrapUp(text);
    } catch (err) {
      onError(`wrap-up: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const enabled = status?.enabled ?? true;
  const actions = status?.action_items ?? [];
  const decisions = status?.decisions ?? [];
  const events = status?.events ?? [];

  return (
    <div
      className="brain-pane"
      style={{ display: "flex", flexDirection: "column", height: "100%", minWidth: 0 }}
    >
      <div className="forge-header">
        <span className="pane-title">🧠 Brain</span>
        <div
          className="forge-actions"
          style={{ display: "flex", gap: 8, alignItems: "center" }}
        >
          <label
            style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 12 }}
            title="Run the 2nd brain during meetings"
          >
            <input
              type="checkbox"
              checked={enabled}
              onChange={(e) => toggle(e.target.checked)}
              style={{ width: "auto", margin: 0 }}
            />
            On
          </label>
          <button onClick={doWrapUp} disabled={busy || !meeting}>
            {busy ? "…" : "Wrap up"}
          </button>
          <button className="close-btn" onClick={onCollapse} title="Collapse">
            ×
          </button>
        </div>
      </div>

      <div
        className="brain-body"
        style={{ overflowY: "auto", padding: "8px 10px", flex: 1, minHeight: 0 }}
      >
        {!enabled && (
          <p className="muted" style={{ fontSize: 12 }}>
            Brain is off — no meeting content is sent to the agent. Toggle "On"
            to detect action items &amp; decisions live.
          </p>
        )}

        <Section title={`Action items (${actions.length})`}>
          {actions.length === 0 ? (
            <Empty />
          ) : (
            actions.map((a) => (
              <label
                key={a.id}
                style={{
                  display: "flex",
                  gap: 6,
                  alignItems: "flex-start",
                  margin: "4px 0",
                  fontSize: 13,
                  opacity: a.done ? 0.5 : 1,
                }}
              >
                <input
                  type="checkbox"
                  checked={a.done}
                  onChange={() => !a.done && markDone(a.id)}
                  style={{ width: "auto", marginTop: 3 }}
                />
                <span style={{ textDecoration: a.done ? "line-through" : "none" }}>
                  {a.assignee ? <strong>{a.assignee}: </strong> : null}
                  {a.text}
                </span>
              </label>
            ))
          )}
        </Section>

        <Section title={`Decisions (${decisions.length})`}>
          {decisions.length === 0 ? (
            <Empty />
          ) : (
            decisions.map((d) => (
              <div key={d.id} style={{ margin: "4px 0", fontSize: 13 }}>
                ✓ {d.text}
              </div>
            ))
          )}
        </Section>

        <Section title={`Live feed (${events.length})`}>
          {events.length === 0 ? (
            <Empty />
          ) : (
            [...events]
              .slice(-30)
              .reverse()
              .map((ev, i) => (
                <div key={i} style={{ margin: "3px 0", fontSize: 12 }}>
                  <span className="muted">{kindIcon(ev.kind)} </span>
                  {ev.content}
                </div>
              ))
          )}
        </Section>

        {wrapUp && (
          <Section title="Wrap-up">
            <pre style={{ whiteSpace: "pre-wrap", fontSize: 12.5, lineHeight: 1.4 }}>
              {wrapUp}
            </pre>
          </Section>
        )}
      </div>
    </div>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div
        style={{
          fontSize: 11,
          textTransform: "uppercase",
          letterSpacing: 0.5,
          opacity: 0.6,
          marginBottom: 4,
        }}
      >
        {title}
      </div>
      {children}
    </div>
  );
}

function Empty() {
  return (
    <div className="muted" style={{ fontSize: 12, fontStyle: "italic" }}>
      nothing yet
    </div>
  );
}

function kindIcon(kind: string): string {
  switch (kind) {
    case "action_item":
      return "📌";
    case "decision":
      return "✓";
    case "context_recall":
      return "🔗";
    default:
      return "•";
  }
}
