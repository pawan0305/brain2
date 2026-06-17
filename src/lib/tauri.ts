import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AudioLevel,
  BrainStatus,
  DgStatusPayload,
  Meeting,
  MeetingCost,
  MeetingSummaryRow,
  SettingsView,
} from "./types";

export const api = {
  getSettings: () => invoke<SettingsView>("get_settings"),
  setApiKeys: (deepgram?: string, anthropic?: string) =>
    invoke<SettingsView>("set_api_keys", { deepgram, anthropic }),
  setTranslateEnabled: (enabled: boolean) =>
    invoke<SettingsView>("set_translate_enabled", { enabled }),
  setCaptureMic: (enabled: boolean) =>
    invoke<SettingsView>("set_capture_mic", { enabled }),
  setOverlayMode: (mode: "off" | "dual" | "en") =>
    invoke<SettingsView>("set_overlay_mode", { mode }),
  setOverlayFontSize: (size: number) =>
    invoke<SettingsView>("set_overlay_font_size", { size }),
  setOverlayLocked: (locked: boolean) =>
    invoke<SettingsView>("set_overlay_locked", { locked }),
  setVocab: (words: string[]) =>
    invoke<SettingsView>("set_vocab", { words }),
  setTargetLanguage: (language: string) =>
    invoke<SettingsView>("set_target_language", { language }),
  setSourceLanguage: (code: string) =>
    invoke<SettingsView>("set_source_language", { code }),
  setLlmProvider: (provider: "anthropic" | "openai") =>
    invoke<SettingsView>("set_llm_provider", { provider }),
  setOpenAIConfig: (cfg: {
    apiKey?: string;
    baseUrl?: string;
    model?: string;
  }) =>
    invoke<SettingsView>("set_openai_config", {
      apiKey: cfg.apiKey,
      baseUrl: cfg.baseUrl,
      model: cfg.model,
    }),
  saveOverlayGeometry: (x: number, y: number, w: number, h: number) =>
    invoke<void>("save_overlay_geometry", { x, y, w, h }),
  setMeetingNotes: (id: string | undefined, notes: string) =>
    invoke<void>("set_meeting_notes", { id, notes }),
  setMeetingTags: (id: string | undefined, tags: string[]) =>
    invoke<void>("set_meeting_tags", { id, tags }),
  startMeeting: (title?: string) =>
    invoke<Meeting>("start_meeting", { title }),
  stopMeeting: () => invoke<Meeting>("stop_meeting"),
  setPaused: (paused: boolean) => invoke<boolean>("set_paused", { paused }),
  isPaused: () => invoke<boolean>("is_paused"),
  currentMeeting: () => invoke<Meeting | null>("current_meeting"),
  setMeetingTitle: (title: string) =>
    invoke<void>("set_meeting_title", { title }),
  listMeetings: () => invoke<MeetingSummaryRow[]>("list_meetings"),
  loadMeeting: (id: string) => invoke<Meeting>("load_meeting", { id }),
  deleteMeeting: (id: string) => invoke<void>("delete_meeting", { id }),
  renameMeeting: (id: string, title: string) =>
    invoke<void>("rename_meeting", { id, title }),
  mergeMeetings: (source: string, target: string) =>
    invoke<void>("merge_meetings", { source, target }),
  exportEnglishTranscript: (id?: string) =>
    invoke<string>("export_english_transcript", { id }),
  /** Writes the raw transcript (with [HH:MM:SS] + speaker labels) to
   *  ~/Downloads/<title>-raw.txt and returns the absolute path. */
  exportRawTranscriptFile: (id?: string) =>
    invoke<string>("export_raw_transcript_file", { id }),
  /** Cleaned (LLM-fixed transcription errors / jargon / metaphors) +
   *  translated transcript, preserving timestamps + speaker labels.
   *  Writes to ~/Downloads/<title>-cleaned-<lang>.txt and returns the
   *  absolute path. Slower (involves an LLM call over the full transcript). */
  exportCleanedTranslatedTranscriptFile: (id?: string) =>
    invoke<string>("export_cleaned_translated_transcript_file", { id }),
  regenerateSummary: (id?: string) => invoke<void>("regenerate_summary", { id }),
  askQuestion: (question: string, meetingId?: string) =>
    invoke<{ stream_id: string }>("ask_question", {
      question,
      meetingId,
    }),
  // Agent backend — the "brain" harness: direct Haiku, Claude Code, or Hermes.
  setAgentBackend: (backend: "direct" | "claude_code" | "hermes") =>
    invoke<SettingsView>("set_agent_backend", { backend }),
  setClaudeModel: (model: string) =>
    invoke<SettingsView>("set_claude_model", { model }),
  setHermesConfig: (cfg: { provider?: string; model?: string }) =>
    invoke<SettingsView>("set_hermes_config", {
      provider: cfg.provider,
      model: cfg.model,
    }),
  // Brain engine.
  brainStatus: () => invoke<BrainStatus>("brain_status"),
  brainToggle: (enabled: boolean) =>
    invoke<BrainStatus>("brain_toggle", { enabled }),
  brainMarkActionDone: (id: string) =>
    invoke<BrainStatus>("brain_mark_action_done", { id }),
  brainWrapUp: (meetingId: string, meetingTitle: string, fullTranscript: string) =>
    invoke<string>("brain_wrap_up", { meetingId, meetingTitle, fullTranscript }),
};

export type EventHandlers = {
  "meeting:started": Meeting;
  "meeting:stopped": Meeting;
  "meeting:update": Meeting;
  "segment:pending": import("./types").Segment;
  "segment:upsert": import("./types").Segment;
  "segment:translated": { id: string; english: string | null; error?: string };
  "summary:update": { summary: string; updated_at: string };
  "chat:user": { stream_id: string; question: string };
  "chat:delta": { stream_id: string; delta: string };
  "chat:done": { stream_id: string; answer: string };
  "chat:error": { stream_id: string; error: string };
  "dg:status": DgStatusPayload;
  "audio:level": AudioLevel;
  "cost:update": MeetingCost;
  "meeting:paused": { paused: boolean };
  error: { message: string };
};

export function on<K extends keyof EventHandlers>(
  event: K,
  handler: (payload: EventHandlers[K]) => void,
): Promise<UnlistenFn> {
  return listen<EventHandlers[K]>(event, (e) => handler(e.payload));
}
