import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, on } from "./lib/tauri";
import type { AudioLevel, DgStatus, Meeting, MeetingCost, MeetingSummaryRow, Segment, SettingsView } from "./lib/types";
import { TopBar } from "./components/TopBar";
import { TranscriptPane } from "./components/TranscriptPane";
import { SummaryPane } from "./components/SummaryPane";
import { ChatPane } from "./components/ChatPane";
import { NotesPane } from "./components/NotesPane";
import { ForgePane } from "./components/ForgePane";
import { BrainPane } from "./components/BrainPane";
import { SettingsModal } from "./components/SettingsModal";
import { HistoryDrawer } from "./components/HistoryDrawer";
import { Splitter } from "./components/Splitter";
import React from "react";

function usePersistedNumber(key: string, fallback: number): [number, (n: number) => void] {
  const [value, setValue] = useState<number>(() => {
    try {
      const v = localStorage.getItem(key);
      const n = v ? Number(v) : NaN;
      return Number.isFinite(n) ? n : fallback;
    } catch {
      return fallback;
    }
  });
  const set = (n: number) => {
    setValue(n);
    try {
      localStorage.setItem(key, String(n));
    } catch {
      /* noop */
    }
  };
  return [value, set];
}

export function App() {
  const [settings, setSettings] = useState<SettingsView | null>(null);
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [pending, setPending] = useState<Segment | null>(null);
  const [running, setRunning] = useState(false);
  const [streamingChatId, setStreamingChatId] = useState<string | null>(null);
  const [streamingChatText, setStreamingChatText] = useState("");
  const [showSettings, setShowSettings] = useState(false);
  const [showHistory, setShowHistory] = useState(false);
  const [history, setHistory] = useState<MeetingSummaryRow[]>([]);
  const [errors, setErrors] = useState<string[]>([]);
  const [audioLevel, setAudioLevel] = useState<AudioLevel>({ mic: 0, sys: 0 });
  const [dgStatus, setDgStatus] = useState<DgStatus>("disconnected");
  const [cost, setCost] = useState<MeetingCost | null>(null);
  const [paused, setPaused] = useState(false);
  const [agentStatus, setAgentStatus] = useState<
    { state: "warming" | "ready" | "error"; error?: string } | null
  >(null);
  const [stack, setStack] = useState<
    Record<string, { state: "ok" | "starting" | "down"; detail: string }>
  >({});
  const [feedNote, setFeedNote] = useState<string | null>(null);
  const feedTimer = useRef<number | null>(null);

  // Per-pane collapse state — persisted in localStorage.
  const [transcriptCollapsed, setTranscriptCollapsed] =
    usePersistedBool("paneCollapsed.transcript", false);
  const [summaryCollapsed, setSummaryCollapsed] =
    usePersistedBool("paneCollapsed.summary", false);
  const [chatCollapsed, setChatCollapsed] =
    usePersistedBool("paneCollapsed.chat", false);
  const [notesCollapsed, setNotesCollapsed] =
    usePersistedBool("paneCollapsed.notes", true);
  const [forgeCollapsed, setForgeCollapsed] =
    usePersistedBool("paneCollapsed.forge", false);
  const [brainCollapsed, setBrainCollapsed] =
    usePersistedBool("paneCollapsed.brain", true);

  const meetingRef = useRef<Meeting | null>(null);
  meetingRef.current = meeting;

  // Load initial state.
  useEffect(() => {
    (async () => {
      try {
        const s = await api.getSettings();
        setSettings(s);
        if (!s.deepgram_set || !s.anthropic_set) setShowSettings(true);
      } catch (err) {
        pushError(`load settings: ${err}`);
      }
      try {
        const m = await api.currentMeeting();
        if (m) {
          setMeeting(m);
          setRunning(true);
        }
      } catch (err) {
        pushError(`load current: ${err}`);
      }
      try {
        setHistory(await api.listMeetings());
      } catch (err) {
        pushError(`list meetings: ${err}`);
      }
    })();
  }, []);

  const pushError = useCallback((msg: string) => {
    setErrors((prev) => [...prev.slice(-4), msg]);
  }, []);

  // Subscribe to backend events.
  useEffect(() => {
    const unlisteners: Promise<() => void>[] = [];
    unlisteners.push(
      on("meeting:started", (m) => {
        setMeeting(m);
        setRunning(true);
        setPaused(false);
        setPending(null);
        setStreamingChatId(null);
        setStreamingChatText("");
        setCost(null);
        setAudioLevel({ mic: 0, sys: 0 });
      }),
      on("meeting:stopped", (m) => {
        setMeeting(m);
        setRunning(false);
        setPaused(false);
        setPending(null);
        setDgStatus("disconnected");
        setAudioLevel({ mic: 0, sys: 0 });
        api.listMeetings().then(setHistory).catch(() => {});
      }),
      on("meeting:update", (m) => setMeeting(m)),
      on("segment:pending", (seg) => setPending(seg)),
      on("segment:upsert", (seg) => {
        setPending(null);
        setMeeting((prev) => {
          if (!prev) return prev;
          const exists = prev.segments.some((s) => s.id === seg.id);
          const segments = exists
            ? prev.segments.map((s) => (s.id === seg.id ? seg : s))
            : [...prev.segments, seg];
          return { ...prev, segments };
        });
      }),
      on("segment:translated", ({ id, english, error }) => {
        if (error) pushError(error);
        setMeeting((prev) =>
          prev
            ? {
                ...prev,
                segments: prev.segments.map((s) =>
                  s.id === id ? { ...s, english } : s,
                ),
              }
            : prev,
        );
      }),
      on("summary:update", ({ summary, updated_at }) =>
        setMeeting((prev) =>
          prev
            ? { ...prev, summary, summary_updated_at: updated_at }
            : prev,
        ),
      ),
      on("chat:user", ({ stream_id, question }) => {
        setStreamingChatId(stream_id);
        setStreamingChatText("");
        setMeeting((prev) =>
          prev
            ? {
                ...prev,
                chat: [
                  ...prev.chat,
                  { role: "user", content: question, at: new Date().toISOString() },
                ],
              }
            : prev,
        );
      }),
      on("chat:delta", ({ delta }) => {
        setStreamingChatText((prev) => prev + delta);
      }),
      on("chat:done", ({ answer }) => {
        setMeeting((prev) =>
          prev
            ? {
                ...prev,
                chat: [
                  ...prev.chat,
                  {
                    role: "assistant",
                    content: answer,
                    at: new Date().toISOString(),
                  },
                ],
              }
            : prev,
        );
        setStreamingChatId(null);
        setStreamingChatText("");
      }),
      on("chat:error", ({ error }) => {
        pushError(`chat: ${error}`);
        setStreamingChatId(null);
        setStreamingChatText("");
      }),
      on("error", ({ message }) => pushError(message)),
      on("audio:level", (lvl) => setAudioLevel(lvl)),
      on("dg:status", ({ status }) => setDgStatus(status)),
      on("agent:status", (s) => setAgentStatus(s)),
      on("stack:health", (h) =>
        setStack((prev) => ({
          ...prev,
          [h.component]: { state: h.state, detail: h.detail },
        })),
      ),
      on("feed:event", ({ kind, detail }) => {
        const text =
          kind === "meeting:distilling"
            ? `🧠 distilling meeting…`
            : kind === "meeting:done"
              ? `🧠 meeting saved to brain`
              : kind === "project:distilling"
                ? `🧠 summarizing ${detail}…`
                : kind === "project:done"
                  ? `🧠 ${detail} → brain`
                  : kind === "error"
                    ? `⚠ feed: ${detail}`
                    : `🧠 ${kind}`;
        setFeedNote(text);
        if (feedTimer.current) window.clearTimeout(feedTimer.current);
        feedTimer.current = window.setTimeout(() => setFeedNote(null), 6000);
      }),
      on("cost:update", (c) => setCost(c)),
      on("meeting:paused", ({ paused }) => setPaused(paused)),
    );

    return () => {
      unlisteners.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [pushError]);

  const start = useCallback(async () => {
    try {
      const m = await api.startMeeting();
      setMeeting(m);
      setRunning(true);
    } catch (err) {
      pushError(`start: ${err}`);
    }
  }, [pushError]);

  const stop = useCallback(async () => {
    try {
      await api.stopMeeting();
    } catch (err) {
      pushError(`stop: ${err}`);
    }
  }, [pushError]);

  const ask = useCallback(
    async (q: string) => {
      try {
        await api.askQuestion(q, meetingRef.current?.id);
      } catch (err) {
        pushError(`ask: ${err}`);
      }
    },
    [pushError],
  );

  const renameMeeting = useCallback(
    async (title: string) => {
      try {
        await api.setMeetingTitle(title);
      } catch (err) {
        pushError(`rename: ${err}`);
      }
    },
    [pushError],
  );

  const regenerateSummary = useCallback(async () => {
    try {
      await api.regenerateSummary(meetingRef.current?.id);
    } catch (err) {
      pushError(`summary: ${err}`);
    }
  }, [pushError]);

  const openMeeting = useCallback(
    async (id: string) => {
      try {
        const m = await api.loadMeeting(id);
        setMeeting(m);
        setRunning(false);
        setShowHistory(false);
      } catch (err) {
        pushError(`load: ${err}`);
      }
    },
    [pushError],
  );

  const onSaveKeys = useCallback(
    async (dg: string, an: string) => {
      try {
        const s = await api.setApiKeys(
          dg.trim() || undefined,
          an.trim() || undefined,
        );
        setSettings(s);
        if (s.deepgram_set && s.anthropic_set) setShowSettings(false);
      } catch (err) {
        pushError(`save settings: ${err}`);
      }
    },
    [pushError],
  );

  const liveSegments = useMemo(() => {
    if (!meeting) return [];
    if (pending && running) return [...meeting.segments, pending];
    return meeting.segments;
  }, [meeting, pending, running]);

  return (
    <div className="app">
      <TopBar
        meeting={meeting}
        running={running}
        paused={paused}
        audioLevel={audioLevel}
        dgStatus={dgStatus}
        cost={cost ?? meeting?.cost ?? null}
        stack={stack}
        agentStatus={agentStatus}
        feedNote={feedNote}
        onStart={start}
        onStop={stop}
        onTogglePause={async () => {
          try {
            const next = !paused;
            await api.setPaused(next);
            setPaused(next);
          } catch (err) {
            pushError(`pause: ${err}`);
          }
        }}
        onOpenSettings={() => setShowSettings(true)}
        onOpenHistory={() => setShowHistory(true)}
        onRenameMeeting={renameMeeting}
        onToggleTranslate={async (enabled) => {
          try {
            setSettings(await api.setTranslateEnabled(enabled));
          } catch (err) {
            pushError(`translate toggle: ${err}`);
          }
        }}
        onCycleOverlay={async () => {
          const cur = settings?.overlay_mode ?? "off";
          const next = cur === "off" ? "dual" : cur === "dual" ? "en" : "off";
          try {
            setSettings(await api.setOverlayMode(next));
          } catch (err) {
            pushError(`overlay: ${err}`);
          }
        }}
        onChangeOverlayFontSize={async (delta) => {
          const cur = settings?.overlay_font_size ?? 24;
          const next = Math.max(12, Math.min(64, cur + delta));
          try {
            setSettings(await api.setOverlayFontSize(next));
          } catch (err) {
            pushError(`overlay font: ${err}`);
          }
        }}
        onToggleOverlayLocked={async () => {
          try {
            setSettings(await api.setOverlayLocked(!(settings?.overlay_locked ?? true)));
          } catch (err) {
            pushError(`overlay lock: ${err}`);
          }
        }}
        onToggleFeed={async () => {
          try {
            setSettings(await api.setBrainFeedEnabled(!(settings?.brain_feed_enabled ?? true)));
          } catch (err) {
            pushError(`brain feed: ${err}`);
          }
        }}
        settings={settings}
      />
      <ResizableMain
        panes={[
          {
            id: "transcript",
            title: "Transcript",
            collapsed: transcriptCollapsed,
            onToggle: () => setTranscriptCollapsed(!transcriptCollapsed),
            content: (
              <TranscriptPane
                segments={liveSegments}
                pendingId={pending?.id}
                meetingId={meeting?.id}
                showEnglish={settings?.translate ?? true}
                onError={pushError}
                onCollapse={() => setTranscriptCollapsed(true)}
              />
            ),
          },
          {
            id: "summary",
            title: "Summary",
            collapsed: summaryCollapsed,
            onToggle: () => setSummaryCollapsed(!summaryCollapsed),
            content: (
              <SummaryPane
                summary={meeting?.summary ?? null}
                updatedAt={meeting?.summary_updated_at ?? null}
                onRegenerate={meeting ? regenerateSummary : undefined}
                onCollapse={() => setSummaryCollapsed(true)}
              />
            ),
          },
          {
            id: "chat",
            title: "Ask the meeting",
            collapsed: chatCollapsed,
            onToggle: () => setChatCollapsed(!chatCollapsed),
            content: (
              <ChatPane
                history={meeting?.chat ?? []}
                streamingId={streamingChatId}
                streamingText={streamingChatText}
                disabled={!meeting}
                onAsk={ask}
                onCollapse={() => setChatCollapsed(true)}
                agentStatus={agentStatus}
              />
            ),
          },
          {
            id: "notes",
            title: "Notes",
            collapsed: notesCollapsed,
            onToggle: () => setNotesCollapsed(!notesCollapsed),
            content: (
              <NotesPane
                meetingId={meeting?.id}
                notes={meeting?.notes ?? ""}
                disabled={!meeting}
                onError={pushError}
                onCollapse={() => setNotesCollapsed(true)}
              />
            ),
          },
          {
            id: "brain",
            title: "Brain",
            collapsed: brainCollapsed,
            onToggle: () => setBrainCollapsed(!brainCollapsed),
            content: (
              <BrainPane
                meeting={meeting}
                onError={pushError}
                onCollapse={() => setBrainCollapsed(true)}
              />
            ),
          },
          {
            id: "forge",
            title: "Forge",
            collapsed: forgeCollapsed,
            onToggle: () => setForgeCollapsed(!forgeCollapsed),
            content: (
              <ForgePane
                onCollapse={() => setForgeCollapsed(true)}
              />
            ),
          },
        ]}
      />
      {errors.length > 0 && (
        <div className="errors">
          {errors.map((e, i) => (
            <div key={i} className="error">
              {e}
            </div>
          ))}
          <button onClick={() => setErrors([])}>dismiss</button>
        </div>
      )}
      {showSettings && (
        <SettingsModal
          settings={settings}
          onSave={onSaveKeys}
          onSettingsChanged={setSettings}
          onError={pushError}
          onClose={() => setShowSettings(false)}
        />
      )}
      {showHistory && (
        <HistoryDrawer
          rows={history}
          onOpen={openMeeting}
          onClose={() => setShowHistory(false)}
          onRefresh={async () => {
            try {
              setHistory(await api.listMeetings());
            } catch (err) {
              pushError(`history: ${err}`);
            }
          }}
          onDelete={async (id) => {
            try {
              await api.deleteMeeting(id);
              setHistory(await api.listMeetings());
            } catch (err) {
              pushError(`delete: ${err}`);
            }
          }}
          onRename={async (id, title) => {
            try {
              await api.renameMeeting(id, title);
              setHistory(await api.listMeetings());
            } catch (err) {
              pushError(`rename: ${err}`);
            }
          }}
          onMerge={async (source, target) => {
            try {
              await api.mergeMeetings(source, target);
              setHistory(await api.listMeetings());
              // If the user is currently viewing one of the merged meetings,
              // refresh the open view: reload the survivor (target), or clear
              // it if they were viewing the now-deleted source.
              if (meeting) {
                if (meeting.id === source) {
                  setMeeting(null);
                } else if (meeting.id === target) {
                  try {
                    setMeeting(await api.loadMeeting(target));
                  } catch (err) {
                    pushError(`reload: ${err}`);
                  }
                }
              }
            } catch (err) {
              pushError(`merge: ${err}`);
            }
          }}
          onError={pushError}
        />
      )}
    </div>
  );
}

function usePersistedBool(key: string, fallback: boolean): [boolean, (v: boolean) => void] {
  const [value, setValue] = useState<boolean>(() => {
    try {
      const v = localStorage.getItem(key);
      if (v === null) return fallback;
      return v === "1" || v === "true";
    } catch {
      return fallback;
    }
  });
  const set = (v: boolean) => {
    setValue(v);
    try {
      localStorage.setItem(key, v ? "1" : "0");
    } catch {
      /* noop */
    }
  };
  return [value, set];
}

function CollapsedStrip({ title, onExpand }: { title: string; onExpand: () => void }) {
  return (
    <button
      className="pane-collapsed"
      title={`Expand ${title}`}
      onClick={onExpand}
    >
      <span className="pane-collapsed-label">▶ {title}</span>
    </button>
  );
}

interface PaneSpec {
  id: "transcript" | "summary" | "chat" | "notes" | "forge" | "brain";
  title: string;
  content: React.ReactNode;
  collapsed: boolean;
  onToggle: () => void;
}

function ResizableMain({ panes }: { panes: PaneSpec[] }) {
  // Per-pane widths (only meaningful when expanded).
  const [transcriptW, setTranscriptW] = usePersistedNumber("paneW.transcript", 600);
  const [summaryW, setSummaryW] = usePersistedNumber("paneW.summary", 380);
  const [chatW, setChatW] = usePersistedNumber("paneW.chat", 380);
  const [brainW, setBrainW] = usePersistedNumber("paneW.brain", 360);
  const widths: Record<PaneSpec["id"], number> = {
    transcript: transcriptW,
    summary: summaryW,
    chat: chatW,
    notes: 0, // last expanded pane absorbs the rest
    forge: 0,
    brain: brainW,
  };
  const setWidth: Record<PaneSpec["id"], (n: number) => void> = {
    transcript: setTranscriptW,
    summary: setSummaryW,
    chat: setChatW,
    notes: () => {},
    forge: () => {},
    brain: setBrainW,
  };
  const refs = {
    transcript: useRef<HTMLDivElement>(null),
    summary: useRef<HTMLDivElement>(null),
    chat: useRef<HTMLDivElement>(null),
    notes: useRef<HTMLDivElement>(null),
    forge: useRef<HTMLDivElement>(null),
    brain: useRef<HTMLDivElement>(null),
  };

  // Find the index of the LAST expanded pane — that one gets `flex: 1` so it
  // absorbs remaining horizontal space.
  const lastExpandedIdx = (() => {
    for (let i = panes.length - 1; i >= 0; i--) {
      if (!panes[i].collapsed) return i;
    }
    return -1;
  })();

  return (
    <div className="main">
      {panes.map((p, i) => {
        if (p.collapsed) {
          return (
            <CollapsedStrip key={p.id} title={p.title} onExpand={p.onToggle} />
          );
        }
        const isLast = i === lastExpandedIdx;
        // Splitter only sits between two expanded panes — and resizes the
        // expanded pane immediately to its left. If pane i-1 is collapsed
        // (or there is no pane i-1) we skip the splitter.
        const prev = i > 0 ? panes[i - 1] : null;
        const showSplitter = !!prev && !prev.collapsed && !isLast
          ? prev
          : null;
        // Even if isLast (this pane is `flex: 1 1 0`), we still want a
        // splitter on its LEFT edge to let the user resize the previous
        // expanded pane.
        const leftSplitter = prev && !prev.collapsed ? prev : null;
        return (
          <React.Fragment key={p.id}>
            {leftSplitter && (
              <Splitter
                onResize={(w) => setWidth[leftSplitter.id]?.(w)}
                leftPaneRef={refs[leftSplitter.id]}
              />
            )}
            <div
              ref={refs[p.id]}
              className="pane-wrap"
              style={
                isLast
                  ? { flex: "1 1 0" }
                  : { flex: `0 0 ${widths[p.id]}px` }
              }
            >
              {p.content}
            </div>
            {showSplitter && null /* splitter rendered before the next pane */}
          </React.Fragment>
        );
      })}
    </div>
  );
}
