import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Types ────────────────────────────────────

interface ForgeMessage {
  id: string;
  role: "user" | "agent";
  content: string;
  at: string;
}

interface ForgeVersion {
  version: string;
  commit: string;
  built_at: string;
  exe_path: string | null;
}

interface ForgeStatus {
  initialized: boolean;
  repo_url: string;
  branch: string;
  has_pending_changes: boolean;
  pending_files: string[];
  build_status:
    | "idle"
    | "building"
    | { success: { exe_path: string } }
    | { failed: { error: string } };
  versions: ForgeVersion[];
  messages: ForgeMessage[];
}

// ── Component ────────────────────────────────

export function ForgePane({
  onCollapse,
}: {
  onCollapse: () => void;
}) {
  const [status, setStatus] = useState<ForgeStatus | null>(null);
  const [input, setInput] = useState("");
  const [busy, setBusy] = useState(false);
  const [diff, setDiff] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const chatEndRef = useRef<HTMLDivElement>(null);

  // Load initial status
  const refresh = useCallback(async () => {
    try {
      const s = await invoke<ForgeStatus>("forge_status");
      setStatus(s);
    } catch (err) {
      setError(`status: ${err}`);
    }
  }, []);

  useEffect(() => {
    refresh();
    const unlisten = listen<ForgeStatus>("forge:status", (e) => {
      setStatus(e.payload);
    });
    return () => {
      unlisten.then((fn) => fn()).catch(() => {});
    };
  }, [refresh]);

  // Scroll chat to bottom
  useEffect(() => {
    chatEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [status?.messages]);

  // ── Actions ──────────────────────────────

  const doInit = async () => {
    setBusy(true);
    setError(null);
    try {
      const msg = await invoke<string>("forge_init");
      setStatus((prev) =>
        prev
          ? {
              ...prev,
              initialized: true,
              messages: [
                ...prev.messages,
                {
                  id: crypto.randomUUID(),
                  role: "agent",
                  content: msg,
                  at: new Date().toISOString(),
                },
              ],
            }
          : prev,
      );
      await refresh();
    } catch (err) {
      setError(`init: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doChat = async () => {
    if (!input.trim()) return;
    setBusy(true);
    setError(null);
    const msg = input.trim();
    setInput("");
    // Optimistic user message
    setStatus((prev) =>
      prev
        ? {
            ...prev,
            messages: [
              ...prev.messages,
              {
                id: crypto.randomUUID(),
                role: "user",
                content: msg,
                at: new Date().toISOString(),
              },
            ],
          }
        : prev,
    );
    try {
      await invoke<string>("forge_chat", { message: msg });
      await refresh();
    } catch (err) {
      setError(`chat: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doDiff = async () => {
    setBusy(true);
    setError(null);
    try {
      const d = await invoke<string>("forge_diff");
      setDiff(d);
    } catch (err) {
      setError(`diff: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doApprove = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("forge_approve", {
        message: input.trim() || "Improvement from Forge",
      });
      setDiff(null);
      setInput("");
      await refresh();
    } catch (err) {
      setError(`approve: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doReject = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("forge_reject");
      setDiff(null);
      await refresh();
    } catch (err) {
      setError(`reject: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doBuild = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("forge_build");
      await refresh();
    } catch (err) {
      setError(`build: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doInstall = async (exePath: string) => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("forge_install", { exePath });
      await refresh();
    } catch (err) {
      setError(`install: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const doRollback = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke<string>("forge_rollback");
      await refresh();
    } catch (err) {
      setError(`rollback: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  // ── Render helpers ────────────────────────

  const buildStatusLabel = () => {
    if (!status) return "";
    const bs = status.build_status;
    if (bs === "idle") return "Idle";
    if (bs === "building") return "🔄 Building...";
    if (typeof bs === "object" && "success" in bs)
      return `✅ Built: ${bs.success.exe_path}`;
    if (typeof bs === "object" && "failed" in bs)
      return `❌ Failed: ${bs.failed.error.slice(0, 80)}`;
    return "";
  };

  const canBuild =
    status?.initialized &&
    !status.has_pending_changes &&
    status.build_status !== "building";
  const canApprove = status?.has_pending_changes;

  return (
    <div className="forge-pane">
      <div className="forge-header">
        <span className="pane-title">⚒️ Forge</span>
        <div className="forge-actions">
          {!status?.initialized ? (
            <button onClick={doInit} disabled={busy}>
              Init Workspace
            </button>
          ) : (
            <>
              <span className="forge-badge">
                {status.branch} · {status.pending_files.length} file(s) pending
              </span>
              {canApprove && (
                <>
                  <button onClick={doDiff} disabled={busy}>
                    Preview Diff
                  </button>
                  <button
                    className="approve"
                    onClick={doApprove}
                    disabled={busy}
                  >
                    ✅ Approve
                  </button>
                  <button
                    className="reject"
                    onClick={doReject}
                    disabled={busy}
                  >
                    ❌ Reject
                  </button>
                </>
              )}
              {canBuild && (
                <button onClick={doBuild} disabled={busy}>
                  🔨 Build
                </button>
              )}
              {status.versions.length > 0 && (
                <button onClick={doRollback} disabled={busy}>
                  ↩ Rollback
                </button>
              )}
              <button
                className="close-btn"
                onClick={onCollapse}
                title="Collapse"
              >
                ×
              </button>
            </>
          )}
        </div>
      </div>

      <div className="forge-body">
        {/* Build status bar */}
        <div className="forge-status-bar">{buildStatusLabel()}</div>

        {/* Error banner */}
        {error && (
          <div className="forge-error">
            {error}
            <button onClick={() => setError(null)}>×</button>
          </div>
        )}

        {/* Diff preview */}
        {diff && (
          <div className="forge-diff">
            <div className="forge-diff-header">Pending Changes</div>
            <pre className="forge-diff-content">{diff}</pre>
          </div>
        )}

        {/* Chat messages */}
        <div className="forge-chat">
          {(status?.messages ?? []).map((m) => (
            <div key={m.id} className={`forge-msg ${m.role}`}>
              <div className="forge-msg-role">
                {m.role === "user" ? "You" : "Forge"}
              </div>
              <div className="forge-msg-content">
                {m.content.split("\n").map((line, i) => (
                  <p key={i}>{line || "\u00A0"}</p>
                ))}
              </div>
            </div>
          ))}
          <div ref={chatEndRef} />
        </div>

        {/* Build success — offer install */}
        {status?.build_status &&
          typeof status.build_status === "object" &&
          "success" in status.build_status && (
            <div className="forge-install-prompt">
              <button
                onClick={() =>
                  doInstall(
                    (status.build_status as { success: { exe_path: string } })
                      .success.exe_path,
                  )
                }
                disabled={busy}
              >
                ⚡ Install Update & Restart
              </button>
            </div>
          )}

        {/* Input */}
        {status?.initialized && (
          <div className="forge-input">
            <textarea
              value={input}
              onChange={(e) => setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  if (status.has_pending_changes && input.trim()) {
                    doApprove();
                  } else {
                    doChat();
                  }
                }
              }}
              placeholder={
                status.has_pending_changes
                  ? "Commit message (Enter to approve)..."
                  : "What should Brain2 do better? (e.g. 'add dark mode', 'fix VU meter lag')"
              }
              rows={3}
              disabled={busy}
            />
          </div>
        )}
      </div>
    </div>
  );
}
