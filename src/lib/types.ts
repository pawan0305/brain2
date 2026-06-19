export interface Segment {
  id: string;
  started_at: string;
  dutch: string;
  english?: string | null;
  speaker?: string | null;
  speaker_id?: number | null;
  is_final: boolean;
}

export interface ChatMessage {
  role: "user" | "assistant";
  content: string;
  at: string;
}

export interface MeetingCost {
  deepgram_audio_secs: number;
  anthropic_input_tokens: number;
  anthropic_output_tokens: number;
  anthropic_cache_read_tokens: number;
}

export interface Meeting {
  id: string;
  title: string;
  started_at: string;
  ended_at?: string | null;
  segments: Segment[];
  summary?: string | null;
  summary_updated_at?: string | null;
  chat: ChatMessage[];
  notes?: string;
  tags?: string[];
  speaker_names?: Record<string, string>;
  cost?: MeetingCost;
}

export interface SettingsView {
  deepgram_set: boolean;
  anthropic_set: boolean;
  translate: boolean;
  capture_mic: boolean;
  overlay_mode: string; // "off" | "dual" | "en"
  overlay_font_size: number;
  overlay_locked: boolean;
  keywords: string[];
  target_language: string;
  source_language: string; // Deepgram code: "multi" | "nl" | "nl-BE" | "en" | …
  llm_provider: string; // "anthropic" | "openai"
  openai_set: boolean;
  openai_base_url: string;
  openai_model: string;
  agent_backend: string; // "direct" | "claude_code" | "hermes"
  hermes_provider: string;
  hermes_model: string;
  claude_model: string;
  stt_backend: string; // "deepgram" | "local_whisper"
  whisper_model: string;
  brain_feed_enabled: boolean;
  brain_feed_repos: string[];
  brain_feed_interval_mins: number;
  knowledge_dir: string;
}

export interface MeetingSummaryRow {
  id: string;
  title: string;
  started_at: string;
  ended_at?: string | null;
  segment_count: number;
  tags?: string[];
}

export type DgStatus = "connected" | "reconnecting" | "disconnected";

export interface DgStatusPayload {
  status: DgStatus;
  attempt?: number;
  retry_in_ms?: number;
}

export interface AudioLevel {
  mic: number; // 0..1
  sys: number; // 0..1
}

// ── Brain engine ─────────────────────────────

export interface ActionItem {
  id: string;
  text: string;
  assignee?: string | null;
  detected_at: string;
  meeting_id: string;
  done: boolean;
}

export interface Decision {
  id: string;
  text: string;
  context: string;
  detected_at: string;
  meeting_id: string;
}

export interface MemoryThread {
  id: string;
  title: string;
  related_meetings: string[];
  summary: string;
  last_updated: string;
}

export interface BrainEvent {
  kind: string; // "action_item" | "decision" | "context_recall" | "suggestion"
  content: string;
  at: string;
}

export interface BrainStatus {
  action_items: ActionItem[];
  decisions: Decision[];
  threads: MemoryThread[];
  events: BrainEvent[];
  enabled: boolean;
}

export interface LocalModelInfo {
  name: string;
  approx_mb: number;
  downloaded: boolean;
}
