import { useEffect, useRef, useState } from "react";
import type { ChatMessage } from "../lib/types";

interface Props {
  history: ChatMessage[];
  streamingId: string | null;
  streamingText: string;
  disabled: boolean;
  onAsk: (q: string) => void;
  onCollapse?: () => void;
  /** Brain2 Agent warm-up state; null when the Direct backend is selected. */
  agentStatus?: { state: "warming" | "ready" | "error"; error?: string } | null;
}

const AGENT_LABEL: Record<"warming" | "ready" | "error", string> = {
  warming: "Agent warming…",
  ready: "Agent ready",
  error: "Agent error",
};

export function ChatPane({
  history,
  streamingId,
  streamingText,
  disabled,
  onAsk,
  onCollapse,
  agentStatus,
}: Props) {
  const [input, setInput] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [history.length, streamingText]);

  const submit = () => {
    const q = input.trim();
    if (!q || disabled) return;
    onAsk(q);
    setInput("");
  };

  return (
    <section className="pane chat-pane">
      <header className="pane-header">
        <h2>Talk to the brain</h2>
        <div className="pane-sub-row">
          <span className="pane-sub">
            {disabled ? "no meeting loaded" : `${history.length} messages`}
          </span>
          {agentStatus && (
            <span
              className={`agent-pill ${agentStatus.state}`}
              title={
                agentStatus.state === "error"
                  ? agentStatus.error ?? "agent failed to warm up"
                  : "Brain2 Agent answers using your files + this transcript"
              }
            >
              ● {AGENT_LABEL[agentStatus.state]}
            </span>
          )}
          {onCollapse && (
            <button className="ghost" onClick={onCollapse} title="Collapse">
              ◀
            </button>
          )}
        </div>
      </header>
      <div className="pane-body scroll" ref={scrollRef}>
        {history.length === 0 && !streamingId && (
          <div className="empty">
            Ask things like “What did we decide about the timeline?” or
            “Who was assigned the marketing follow-up?”
          </div>
        )}
        {history.map((m, i) => (
          <div key={i} className={`chat-msg ${m.role}`}>
            <div className="chat-role">{m.role === "user" ? "You" : "Assistant"}</div>
            <div className="chat-content">{m.content}</div>
          </div>
        ))}
        {streamingId && (
          <div className="chat-msg assistant streaming">
            <div className="chat-role">Assistant</div>
            <div className="chat-content">
              {streamingText || <em className="muted">thinking…</em>}
            </div>
          </div>
        )}
      </div>
      <form
        className="chat-input"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <input
          type="text"
          placeholder={disabled ? "start a meeting first" : "Ask a question…"}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          disabled={disabled}
        />
        <button
          type="submit"
          disabled={disabled || !input.trim() || !!streamingId}
        >
          {streamingId ? "…" : "Ask"}
        </button>
      </form>
    </section>
  );
}
