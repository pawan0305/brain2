import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/tauri";
import type { MeetingSummaryRow } from "../lib/types";

interface Props {
  rows: MeetingSummaryRow[];
  onOpen: (id: string) => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onMerge: (source: string, target: string) => void;
  onClose: () => void;
  onRefresh: () => void;
  onError?: (msg: string) => void;
}

export function HistoryDrawer({
  rows,
  onOpen,
  onDelete,
  onRename,
  onMerge,
  onClose,
  onRefresh,
  onError,
}: Props) {
  const [armedDeleteId, setArmedDeleteId] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState("");
  const [editingTagsId, setEditingTagsId] = useState<string | null>(null);
  const [tagsDraft, setTagsDraft] = useState("");
  const [query, setQuery] = useState("");
  // Drag state. WKWebView's HTML5 drag-and-drop is unreliable, so we do
  // it ourselves with raw mouse events. `drag` holds the active drag (id,
  // title, cursor pos), `hoverTarget` is the id under the cursor.
  const [drag, setDrag] = useState<
    { id: string; title: string; x: number; y: number } | null
  >(null);
  const [hoverTarget, setHoverTarget] = useState<string | null>(null);
  // `window.confirm` is unreliable inside Tauri's WKWebView (no dialog
  // plugin), so we show an inline confirmation banner instead.
  const [pendingMerge, setPendingMerge] = useState<{
    source: string;
    sourceTitle: string;
    target: string;
    targetTitle: string;
  } | null>(null);
  const dragInfo = useRef<{
    id: string;
    title: string;
    startX: number;
    startY: number;
    started: boolean;
  } | null>(null);

  useEffect(() => {
    onRefresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const startDrag = (
    e: React.MouseEvent<HTMLSpanElement>,
    row: MeetingSummaryRow,
  ) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    dragInfo.current = {
      id: row.id,
      title: row.title,
      startX: e.clientX,
      startY: e.clientY,
      started: false,
    };

    const onMove = (ev: MouseEvent) => {
      const info = dragInfo.current;
      if (!info) return;
      if (!info.started) {
        const dx = Math.abs(ev.clientX - info.startX);
        const dy = Math.abs(ev.clientY - info.startY);
        if (dx < 4 && dy < 4) return;
        info.started = true;
        document.body.style.cursor = "grabbing";
      }
      setDrag({
        id: info.id,
        title: info.title,
        x: ev.clientX,
        y: ev.clientY,
      });
      // Hit-test for drop target.
      const el = document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null;
      const li = el?.closest("li[data-meeting-id]") as HTMLElement | null;
      const tid = li?.dataset.meetingId ?? null;
      setHoverTarget(tid && tid !== info.id ? tid : null);
    };

    const onUp = (ev: MouseEvent) => {
      document.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      const info = dragInfo.current;
      dragInfo.current = null;
      setDrag(null);
      setHoverTarget(null);
      if (!info || !info.started) return;
      const el = document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null;
      const li = el?.closest("li[data-meeting-id]") as HTMLElement | null;
      const tid = li?.dataset.meetingId;
      if (!tid || tid === info.id) return;
      const tgtRow = rows.find((x) => x.id === tid);
      setPendingMerge({
        source: info.id,
        sourceTitle: info.title,
        target: tid,
        targetTitle: tgtRow?.title ?? "that meeting",
      });
    };

    document.addEventListener("mousemove", onMove);
    document.addEventListener("mouseup", onUp);
  };

  useEffect(() => {
    if (!armedDeleteId) return;
    const t = setTimeout(() => setArmedDeleteId(null), 3000);
    return () => clearTimeout(t);
  }, [armedDeleteId]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter((r) => {
      const inTitle = r.title.toLowerCase().includes(q);
      const inTags = (r.tags ?? []).some((t) => t.toLowerCase().includes(q));
      return inTitle || inTags;
    });
  }, [rows, query]);

  const commitTags = async (id: string) => {
    const tags = tagsDraft
      .split(",")
      .map((s) => s.trim())
      .filter((s) => s.length > 0);
    setEditingTagsId(null);
    try {
      await api.setMeetingTags(id, tags);
      onRefresh();
    } catch (err) {
      onError?.(`tags: ${err}`);
    }
  };

  return (
    <div className="drawer-backdrop" onClick={onClose}>
      {drag && (
        <div
          className="drag-ghost"
          style={{ left: drag.x + 12, top: drag.y + 12 }}
        >
          {drag.title}
        </div>
      )}
      <aside className="drawer" onClick={(e) => e.stopPropagation()}>
        <header className="drawer-header">
          <h2>Meeting history</h2>
          <button onClick={onClose}>✕</button>
        </header>
        <div className="drawer-search">
          <input
            type="text"
            placeholder="Search title or tag…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>
        <div className="drawer-hint">
          Grab ⋮⋮ and drop onto another meeting to combine them.
        </div>
        {pendingMerge && (
          <div className="merge-confirm">
            <div className="merge-confirm-text">
              Merge <strong>{pendingMerge.sourceTitle}</strong> into{" "}
              <strong>{pendingMerge.targetTitle}</strong>?{" "}
              <span className="muted">
                Source will be deleted; summary will be cleared.
              </span>
            </div>
            <div className="merge-confirm-actions">
              <button
                className="primary"
                onClick={() => {
                  onMerge(pendingMerge.source, pendingMerge.target);
                  setPendingMerge(null);
                }}
              >
                Merge
              </button>
              <button onClick={() => setPendingMerge(null)}>Cancel</button>
            </div>
          </div>
        )}
        {visible.length === 0 && (
          <div className="empty">
            {rows.length === 0 ? "No saved meetings yet." : "No matches."}
          </div>
        )}
        <ul className="history-list">
          {visible.map((r) => {
            const armed = armedDeleteId === r.id;
            const renaming = renamingId === r.id;
            const editingTags = editingTagsId === r.id;
            const isDragging = drag?.id === r.id;
            const isDropTarget =
              hoverTarget === r.id && drag != null && drag.id !== r.id;
            return (
              <li
                key={r.id}
                data-meeting-id={r.id}
                className={
                  (isDragging ? "history-dragging" : "") +
                  (isDropTarget ? " history-drop-target" : "")
                }
              >
                <span
                  className="drag-handle"
                  title="Drag onto another meeting to combine"
                  onMouseDown={(e) => {
                    if (renaming || editingTags) return;
                    startDrag(e, r);
                  }}
                  onClick={(e) => e.stopPropagation()}
                >
                  ⋮⋮
                </span>
                {renaming ? (
                  <input
                    className="title-input history-rename"
                    autoFocus
                    value={renameDraft}
                    onChange={(e) => setRenameDraft(e.target.value)}
                    onClick={(e) => e.stopPropagation()}
                    onBlur={() => {
                      const v = renameDraft.trim();
                      if (v && v !== r.title) onRename(r.id, v);
                      setRenamingId(null);
                    }}
                    onKeyDown={(e) => {
                      if (e.key === "Enter")
                        (e.target as HTMLInputElement).blur();
                      if (e.key === "Escape") setRenamingId(null);
                    }}
                  />
                ) : (
                  <button
                    className="history-row"
                    onClick={() => onOpen(r.id)}
                  >
                    <div className="history-title">{r.title}</div>
                    <div className="history-meta">
                      {new Date(r.started_at).toLocaleString()} ·{" "}
                      {r.segment_count} segments
                    </div>
                    {editingTags ? (
                      <input
                        className="history-tags-input"
                        autoFocus
                        value={tagsDraft}
                        onChange={(e) => setTagsDraft(e.target.value)}
                        onClick={(e) => e.stopPropagation()}
                        onBlur={() => commitTags(r.id)}
                        onKeyDown={(e) => {
                          e.stopPropagation();
                          if (e.key === "Enter")
                            (e.target as HTMLInputElement).blur();
                          if (e.key === "Escape") setEditingTagsId(null);
                        }}
                        placeholder="tags, comma-separated"
                      />
                    ) : (
                      <div
                        className="history-tags"
                        onClick={(e) => {
                          e.stopPropagation();
                          setTagsDraft((r.tags ?? []).join(", "));
                          setEditingTagsId(r.id);
                        }}
                        title="Click to edit tags"
                      >
                        {(r.tags ?? []).length === 0
                          ? <span className="muted">+ tags</span>
                          : (r.tags ?? []).map((t) => (
                              <span key={t} className="tag-pill">{t}</span>
                            ))}
                      </div>
                    )}
                  </button>
                )}
                <button
                  className="ghost"
                  title="rename"
                  onClick={() => {
                    setArmedDeleteId(null);
                    setRenameDraft(r.title);
                    setRenamingId(r.id);
                  }}
                >
                  ✏︎
                </button>
                <button
                  className="ghost danger"
                  title={armed ? "click again to confirm" : "delete"}
                  onClick={() => {
                    if (armed) {
                      setArmedDeleteId(null);
                      onDelete(r.id);
                    } else {
                      setArmedDeleteId(r.id);
                    }
                  }}
                >
                  {armed ? "delete?" : "🗑"}
                </button>
              </li>
            );
          })}
        </ul>
      </aside>
    </div>
  );
}
