import { useEffect, useState } from "react";
import { api } from "../lib/tauri";
import type { LocalModelInfo, SettingsView } from "../lib/types";

// Curated short list of common Claude output languages. Users can type
// anything they want via the "Other…" option, but these cover ~95% of
// cases without scrolling through a hundred locales.
const LANG_OPTIONS = [
  "English",
  "Dutch",
  "Spanish",
  "French",
  "German",
  "Italian",
  "Portuguese",
  "Polish",
  "Russian",
  "Ukrainian",
  "Turkish",
  "Arabic",
  "Hindi",
  "Chinese (Simplified)",
  "Chinese (Traditional)",
  "Japanese",
  "Korean",
  "Indonesian",
  "Vietnamese",
  "Thai",
];

// Source language = the Deepgram `language` param. "multi" auto-detects
// across Nova-3's multilingual set; a specific code locks to one language
// (more accurate when you know what's being spoken). Label → code.
const SOURCE_LANG_OPTIONS: { label: string; code: string }[] = [
  { label: "Auto-detect (multilingual)", code: "multi" },
  { label: "Dutch", code: "nl" },
  { label: "Flemish (Belgian Dutch)", code: "nl-BE" },
  { label: "English", code: "en" },
  { label: "German", code: "de" },
  { label: "French", code: "fr" },
  { label: "Spanish", code: "es" },
  { label: "Italian", code: "it" },
  { label: "Portuguese", code: "pt" },
  { label: "Russian", code: "ru" },
  { label: "Hindi", code: "hi" },
  { label: "Japanese", code: "ja" },
];

interface Props {
  settings: SettingsView | null;
  onSave: (deepgram: string, anthropic: string) => Promise<void> | void;
  onSettingsChanged: (s: SettingsView) => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

export function SettingsModal({ settings, onSave, onSettingsChanged, onClose, onError }: Props) {
  const [dg, setDg] = useState("");
  const [an, setAn] = useState("");
  const [saving, setSaving] = useState(false);
  // Vocab state — one term per line in the textarea.
  const [vocabText, setVocabText] = useState<string>(
    (settings?.keywords ?? []).join("\n"),
  );
  const [savingVocab, setSavingVocab] = useState(false);
  // Target language for Claude (translation/summary/chat). Source is
  // auto-detected by Deepgram.
  const [targetLang, setTargetLang] = useState<string>(
    settings?.target_language || "English",
  );
  const [savingLang, setSavingLang] = useState(false);
  // Source language (Deepgram code). "multi" = auto-detect.
  const [sourceLang, setSourceLang] = useState<string>(
    settings?.source_language || "multi",
  );

  // Capture the mic (your own voice) alongside system audio. Off = system
  // audio only — the fix for speaker users, where the mic re-captures the
  // speaker output and the transcript comes out doubled.
  const [captureMic, setCaptureMic] = useState<boolean>(
    settings?.capture_mic ?? true,
  );
  const saveCaptureMic = async (next: boolean) => {
    setCaptureMic(next);
    try {
      const s = await api.setCaptureMic(next);
      onSettingsChanged(s);
    } catch (err) {
      onError(`capture mic: ${err}`);
    }
  };

  const saveSourceLang = async (code: string) => {
    setSourceLang(code);
    try {
      const s = await api.setSourceLanguage(code);
      onSettingsChanged(s);
    } catch (err) {
      onError(`source language: ${err}`);
    }
  };

  // LLM backend. "anthropic" or "openai" (= any OpenAI-compatible endpoint
  // — OpenAI itself, Ollama, LM Studio, vLLM, OpenRouter, etc.).
  const [llmProvider, setLlmProvider] = useState<string>(
    settings?.llm_provider || "anthropic",
  );
  const [openaiKey, setOpenaiKey] = useState("");
  const [openaiBase, setOpenaiBase] = useState<string>(
    settings?.openai_base_url || "",
  );
  const [openaiModel, setOpenaiModel] = useState<string>(
    settings?.openai_model || "",
  );
  const [savingOpenai, setSavingOpenai] = useState(false);

  // Agent backend — the "brain" harness driving the Brain engine + Forge.
  const [agentBackend, setAgentBackend] = useState<string>(
    settings?.agent_backend || "direct",
  );
  const [claudeModel, setClaudeModel] = useState<string>(
    settings?.claude_model || "haiku",
  );
  const [hermesProvider, setHermesProvider] = useState<string>(
    settings?.hermes_provider || "",
  );
  const [hermesModel, setHermesModel] = useState<string>(
    settings?.hermes_model || "",
  );
  const saveAgentBackend = async (next: string) => {
    setAgentBackend(next);
    try {
      onSettingsChanged(
        await api.setAgentBackend(next as "direct" | "claude_code" | "hermes"),
      );
    } catch (err) {
      onError(`agent backend: ${err}`);
    }
  };
  const saveClaudeModel = async () => {
    try {
      onSettingsChanged(await api.setClaudeModel(claudeModel.trim() || "haiku"));
    } catch (err) {
      onError(`claude model: ${err}`);
    }
  };
  const saveHermesConfig = async () => {
    try {
      onSettingsChanged(
        await api.setHermesConfig({ provider: hermesProvider, model: hermesModel }),
      );
    } catch (err) {
      onError(`hermes config: ${err}`);
    }
  };

  // Speech-to-text engine — cloud Deepgram vs on-device Whisper.
  const [sttBackend, setSttBackend] = useState<string>(
    settings?.stt_backend || "deepgram",
  );
  const [whisperModel, setWhisperModel] = useState<string>(
    settings?.whisper_model || "large-v3-q5_0",
  );
  const [localModels, setLocalModels] = useState<LocalModelInfo[]>([]);
  const [downloadingModel, setDownloadingModel] = useState<string | null>(null);

  const loadModels = async () => {
    try {
      setLocalModels(await api.listLocalModels());
    } catch (err) {
      onError(`list models: ${err}`);
    }
  };
  useEffect(() => {
    if (sttBackend === "local_whisper") loadModels();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveSttBackend = async (next: string) => {
    setSttBackend(next);
    try {
      onSettingsChanged(
        await api.setSttBackend(next as "deepgram" | "local_whisper"),
      );
      if (next === "local_whisper") loadModels();
    } catch (err) {
      onError(`stt backend: ${err}`);
    }
  };
  const saveWhisperModel = async (next: string) => {
    setWhisperModel(next);
    try {
      onSettingsChanged(await api.setWhisperModel(next));
    } catch (err) {
      onError(`whisper model: ${err}`);
    }
  };
  const doDownloadModel = async (name: string) => {
    setDownloadingModel(name);
    try {
      await api.downloadModel(name);
      await loadModels();
    } catch (err) {
      onError(`download ${name}: ${err}`);
    } finally {
      setDownloadingModel(null);
    }
  };

  const saveLlmProvider = async (next: "anthropic" | "openai") => {
    setLlmProvider(next);
    try {
      const s = await api.setLlmProvider(next);
      onSettingsChanged(s);
    } catch (err) {
      onError(`llm provider: ${err}`);
    }
  };

  const saveOpenai = async () => {
    setSavingOpenai(true);
    try {
      const s = await api.setOpenAIConfig({
        apiKey: openaiKey || undefined, // undefined = leave existing untouched
        baseUrl: openaiBase,
        model: openaiModel,
      });
      onSettingsChanged(s);
      setOpenaiKey(""); // clear the password field after save
    } catch (err) {
      onError(`openai config: ${err}`);
    } finally {
      setSavingOpenai(false);
    }
  };

  const submit = async () => {
    setSaving(true);
    try {
      await onSave(dg, an);
    } finally {
      setSaving(false);
    }
  };

  const saveLang = async (next: string) => {
    setTargetLang(next);
    setSavingLang(true);
    try {
      const s = await api.setTargetLanguage(next);
      onSettingsChanged(s);
    } catch (err) {
      onError(`language: ${err}`);
    } finally {
      setSavingLang(false);
    }
  };

  const saveVocab = async () => {
    setSavingVocab(true);
    try {
      const words = vocabText
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
      const s = await api.setVocab(words);
      onSettingsChanged(s);
    } catch (err) {
      onError(`vocab: ${err}`);
    } finally {
      setSavingVocab(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Settings</h2>
        <p className="muted">
          API keys live in <code>%APPDATA%\com.onetruedutchie.app</code>; nothing
          leaves your machine except provider API requests.
        </p>

        <label>
          <span>
            Deepgram API key
            {settings?.deepgram_set && (
              <em className="muted"> (currently set, leave blank to keep)</em>
            )}
          </span>
          <input
            type="password"
            value={dg}
            onChange={(e) => setDg(e.target.value)}
            placeholder="dg_..."
            autoComplete="off"
          />
          <small>
            <a
              href="https://console.deepgram.com/"
              target="_blank"
              rel="noreferrer"
            >
              console.deepgram.com
            </a>{" "}
            · ~$0.0043/min for Nova-3 streaming
          </small>
        </label>

        <label>
          <span>
            Anthropic API key
            {settings?.anthropic_set && (
              <em className="muted"> (currently set, leave blank to keep)</em>
            )}
          </span>
          <input
            type="password"
            value={an}
            onChange={(e) => setAn(e.target.value)}
            placeholder="sk-ant-..."
            autoComplete="off"
          />
          <small>
            <a
              href="https://console.anthropic.com/"
              target="_blank"
              rel="noreferrer"
            >
              console.anthropic.com
            </a>{" "}
            · Claude Haiku 4.5 ($1/MTok in, $5/MTok out)
          </small>
        </label>

        <label>
          <span>
            LLM backend
            <em className="muted"> (for translation, summary, chat)</em>
          </span>
          <select
            value={llmProvider}
            onChange={(e) =>
              saveLlmProvider(e.target.value as "anthropic" | "openai")
            }
          >
            <option value="anthropic">Anthropic (Claude Haiku 4.5)</option>
            <option value="openai">OpenAI-compatible (OpenAI, Ollama, LM Studio…)</option>
          </select>
          <small>
            Anthropic = cloud, prompt caching, ~$0.07/hr of meeting.
            OpenAI-compatible = bring your own — point at OpenAI, or a
            local model on localhost for $0 LLM cost.
          </small>
        </label>

        {llmProvider === "openai" && (
          <label>
            <span>
              OpenAI-compatible endpoint
              {settings?.openai_set && (
                <em className="muted"> (key currently set, leave blank to keep)</em>
              )}
            </span>
            <input
              type="text"
              value={openaiBase}
              onChange={(e) => setOpenaiBase(e.target.value)}
              placeholder="https://api.openai.com/v1  ·  http://localhost:11434/v1 (Ollama)"
              autoComplete="off"
            />
            <input
              type="text"
              value={openaiModel}
              onChange={(e) => setOpenaiModel(e.target.value)}
              placeholder="gpt-4o-mini  ·  llama3.1:8b  ·  qwen2.5:14b"
              autoComplete="off"
              style={{ marginTop: 6 }}
            />
            <input
              type="password"
              value={openaiKey}
              onChange={(e) => setOpenaiKey(e.target.value)}
              placeholder="API key (leave blank for a local model without auth)"
              autoComplete="off"
              style={{ marginTop: 6 }}
            />
            <small>
              For Ollama: base = <code>http://localhost:11434/v1</code>,
              model = whatever you have pulled, key blank.
              For OpenAI: base = <code>https://api.openai.com/v1</code>,
              model = <code>gpt-4o-mini</code>, key from{" "}
              <a href="https://platform.openai.com/api-keys" target="_blank" rel="noreferrer">
                platform.openai.com
              </a>
              .
            </small>
            <div style={{ marginTop: 6 }}>
              <button className="ghost" onClick={saveOpenai} disabled={savingOpenai}>
                {savingOpenai ? "Saving…" : "Save endpoint"}
              </button>
            </div>
          </label>
        )}

        <label>
          <span>
            Brain2 agent
            <em className="muted"> (drives the Brain engine + Forge)</em>
          </span>
          <select
            value={agentBackend}
            onChange={(e) => saveAgentBackend(e.target.value)}
          >
            <option value="direct">Direct — Claude Haiku (fast, default)</option>
            <option value="claude_code">Claude Code (the agent IS Brain2)</option>
            <option value="hermes">Hermes (WSL — local-LLM capable)</option>
          </select>
          <small>
            The brain behind action items, decisions, recall, wrap-ups, and the
            Forge self-improver. Claude Code and Hermes both read the shared
            persona at{" "}
            <code>%LOCALAPPDATA%\com.brain2.app\agent-prompts\BRAIN2.md</code>.
            Live translation always uses the fast Direct path regardless.
          </small>
          {agentBackend === "claude_code" && (
            <div style={{ marginTop: 6 }}>
              <input
                type="text"
                value={claudeModel}
                onChange={(e) => setClaudeModel(e.target.value)}
                onBlur={saveClaudeModel}
                placeholder="haiku · sonnet · opus · or a full model id"
                autoComplete="off"
              />
              <small>
                Claude Code's own default model can be one your account can't
                use headlessly — keep an explicit model here. "haiku" is
                cheapest.
              </small>
            </div>
          )}
          {agentBackend === "hermes" && (
            <div style={{ marginTop: 6 }}>
              <input
                type="text"
                value={hermesProvider}
                onChange={(e) => setHermesProvider(e.target.value)}
                onBlur={saveHermesConfig}
                placeholder="provider (blank = Hermes default; e.g. ollama for local)"
                autoComplete="off"
              />
              <input
                type="text"
                value={hermesModel}
                onChange={(e) => setHermesModel(e.target.value)}
                onBlur={saveHermesConfig}
                placeholder="model (blank = Hermes default)"
                autoComplete="off"
                style={{ marginTop: 6 }}
              />
              <small>
                Runs <code>hermes -z</code> in WSL. Set provider/model to point
                the brain at a local LLM (e.g. provider <code>ollama</code>).
                Requires WSL + Hermes installed.
              </small>
            </div>
          )}
        </label>

        <label>
          <span>
            Speech-to-text
            <em className="muted"> (transcription engine)</em>
          </span>
          <select value={sttBackend} onChange={(e) => saveSttBackend(e.target.value)}>
            <option value="deepgram">Deepgram (cloud, sub-second live)</option>
            <option value="local_whisper">Local Whisper (on-device, private)</option>
          </select>
          <small>
            Deepgram = cloud, lowest latency. Local Whisper runs whisper.cpp on
            your GPU — fully offline (audio never leaves the device) and more
            accurate, but transcripts land ~1-3 s after each pause. Requires a
            build with the <code>local-stt</code> feature.
          </small>
          {sttBackend === "local_whisper" && (
            <div style={{ marginTop: 6 }}>
              <select
                value={whisperModel}
                onChange={(e) => saveWhisperModel(e.target.value)}
              >
                {localModels.length === 0 && (
                  <option value={whisperModel}>{whisperModel}</option>
                )}
                {localModels.map((m) => (
                  <option key={m.name} value={m.name}>
                    {m.name} (~{m.approx_mb} MB){m.downloaded ? " ✓" : " ⬇"}
                  </option>
                ))}
              </select>
              <div style={{ marginTop: 6 }}>
                <button
                  className="ghost"
                  onClick={() => doDownloadModel(whisperModel)}
                  disabled={
                    downloadingModel !== null ||
                    (localModels.find((m) => m.name === whisperModel)?.downloaded ??
                      false)
                  }
                >
                  {downloadingModel === whisperModel
                    ? "Downloading… (large, please wait)"
                    : localModels.find((m) => m.name === whisperModel)?.downloaded
                      ? "Downloaded ✓"
                      : "Download model"}
                </button>
              </div>
              <small>
                Models download once to app-data. large-v3-q5_0 (~1 GB) is the
                accuracy default; pick a smaller one for weaker hardware.
              </small>
            </div>
          )}
        </label>

        <label>
          <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
            <input
              type="checkbox"
              checked={captureMic}
              onChange={(e) => saveCaptureMic(e.target.checked)}
              style={{ width: "auto", margin: 0 }}
            />
            Capture microphone
            <em className="muted"> (your own voice)</em>
          </span>
          <small>
            Mixes your mic into the transcript along with the system audio.{" "}
            <strong>Turn this off if you listen on speakers</strong> — the mic
            also picks up the system audio coming out of the speakers, so
            everything gets transcribed twice. On headphones, leave it on.
            Takes effect on the next meeting.
          </small>
        </label>

        <label>
          <span>
            Source language
            <em className="muted"> (what's being spoken)</em>
          </span>
          <select
            value={sourceLang}
            onChange={(e) => saveSourceLang(e.target.value)}
          >
            {SOURCE_LANG_OPTIONS.map((o) => (
              <option key={o.code} value={o.code}>{o.label}</option>
            ))}
          </select>
          <small>
            Locking to one language (e.g. Dutch) is noticeably more
            accurate than auto-detect when you know what's being spoken.
            Use auto-detect for mixed-language calls. Takes effect on the
            next meeting.
          </small>
        </label>

        <label>
          <span>
            Target language
            <em className="muted"> (translation, summary, chat output)</em>
          </span>
          <select
            value={LANG_OPTIONS.includes(targetLang) ? targetLang : "__custom"}
            onChange={(e) => {
              if (e.target.value === "__custom") return;
              saveLang(e.target.value);
            }}
            disabled={savingLang}
          >
            {LANG_OPTIONS.map((l) => (
              <option key={l} value={l}>{l}</option>
            ))}
            <option value="__custom">Other…</option>
          </select>
          {(!LANG_OPTIONS.includes(targetLang) || targetLang === "") && (
            <input
              type="text"
              value={targetLang}
              onChange={(e) => setTargetLang(e.target.value)}
              onBlur={() => saveLang(targetLang)}
              placeholder="e.g. Vietnamese, Brazilian Portuguese"
              style={{ marginTop: 6 }}
              autoComplete="off"
            />
          )}
          <small>
            Source language is auto-detected. This sets the language Claude
            translates / summarises into. Takes effect on the next call.
          </small>
        </label>

        <label>
          <span>
            Custom vocabulary
            <em className="muted"> (one word/phrase per line)</em>
          </span>
          <textarea
            value={vocabText}
            onChange={(e) => setVocabText(e.target.value)}
            placeholder={"names, jargon, or words Deepgram keeps mishearing\ne.g.\nKlaas\nDigiD\nABN Amro"}
            rows={5}
            spellCheck={false}
            autoComplete="off"
          />
          <small>
            Boosts these terms in Deepgram (Nova-3 <code>keyterm</code>). Takes
            effect on the next meeting.
          </small>
          <div style={{ marginTop: 6 }}>
            <button
              className="ghost"
              onClick={saveVocab}
              disabled={savingVocab}
            >
              {savingVocab ? "Saving…" : "Save vocabulary"}
            </button>
          </div>
        </label>

        <div className="modal-actions">
          <button onClick={onClose}>Close</button>
          <button className="primary" onClick={submit} disabled={saving}>
            {saving ? "Saving…" : "Save keys"}
          </button>
        </div>
      </div>
    </div>
  );
}
