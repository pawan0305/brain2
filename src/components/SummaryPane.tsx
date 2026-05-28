interface Props {
  summary: string | null;
  updatedAt: string | null;
  onRegenerate?: () => void;
  onCollapse?: () => void;
}

export function SummaryPane({ summary, updatedAt, onRegenerate, onCollapse }: Props) {
  return (
    <section className="pane summary-pane">
      <header className="pane-header">
        <h2>Summary</h2>
        <div className="pane-sub-row">
          {updatedAt && (
            <span className="pane-sub">
              updated {new Date(updatedAt).toLocaleTimeString()}
            </span>
          )}
          {onRegenerate && (
            <button className="ghost" onClick={onRegenerate}>
              {summary ? "↻ regenerate" : "Generate"}
            </button>
          )}
          {onCollapse && (
            <button className="ghost" onClick={onCollapse} title="Collapse">
              ◀
            </button>
          )}
        </div>
      </header>
      <div className="pane-body scroll">
        {summary ? (
          <pre className="summary-text">{summary}</pre>
        ) : (
          <div className="empty">
            No summary yet — click <strong>Generate</strong> when you want one.
          </div>
        )}
      </div>
    </section>
  );
}
