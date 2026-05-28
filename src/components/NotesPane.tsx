import { useEffect, useRef, useState } from "react";
import { api } from "../lib/tauri";

interface Props {
  meetingId: string | undefined;
  notes: string;
  disabled?: boolean;
  onError?: (msg: string) => void;
  onCollapse?: () => void;
}

const SAVE_DELAY_MS = 600;

export function NotesPane({ meetingId, notes, disabled, onError, onCollapse }: Props) {
  // Local working copy so typing stays snappy; we debounce-save to backend.
  const [value, setValue] = useState(notes);
  const savedRef = useRef(notes);
  const timerRef = useRef<number | null>(null);
  const [dirty, setDirty] = useState(false);

  // External update (load meeting / meeting:update event) — reset local
  // copy unless the user is mid-edit (dirty).
  useEffect(() => {
    if (!dirty && notes !== savedRef.current) {
      savedRef.current = notes;
      setValue(notes);
    }
  }, [notes, dirty]);

  const scheduleSave = (next: string) => {
    setValue(next);
    setDirty(true);
    if (timerRef.current) window.clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(async () => {
      try {
        await api.setMeetingNotes(meetingId, next);
        savedRef.current = next;
        setDirty(false);
      } catch (err) {
        onError?.(`notes: ${err}`);
      }
    }, SAVE_DELAY_MS);
  };

  useEffect(() => {
    return () => {
      if (timerRef.current) window.clearTimeout(timerRef.current);
    };
  }, []);

  return (
    <section className="pane notes-pane">
      <header className="pane-header">
        <h2>Notes</h2>
        <div className="pane-sub-row">
          <span className="pane-sub">{dirty ? "saving…" : value ? "saved" : ""}</span>
          {onCollapse && (
            <button className="ghost" onClick={onCollapse} title="Collapse">
              ◀
            </button>
          )}
        </div>
      </header>
      <div className="pane-body">
        <textarea
          className="notes-textarea"
          value={value}
          disabled={disabled}
          placeholder={
            disabled
              ? "Start or open a meeting to take notes"
              : "Your private notes for this meeting — saved automatically."
          }
          onChange={(e) => scheduleSave(e.target.value)}
        />
      </div>
    </section>
  );
}
