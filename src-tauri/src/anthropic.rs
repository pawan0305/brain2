use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
pub const MODEL_HAIKU: &str = "claude-haiku-4-5-20251001";

pub struct AnthropicClient {
    api_key: String,
    http: reqwest::Client,
    model: String,
    /// Target language for translation / summary / chat output (e.g. "English",
    /// "Spanish", "Japanese"). Source is whatever Deepgram detects.
    target_language: String,
}

#[derive(Debug, Clone)]
pub enum ChatStreamEvent {
    Delta(String),
    Done(String),
    Error(String),
}

#[derive(Debug, Serialize)]
struct MessageReq<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemBlock<'a>>>,
    messages: Vec<MessageItem<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SystemBlock<'a> {
    Text {
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Serialize)]
struct CacheControl {
    #[serde(rename = "type")]
    typ: &'static str,
}

#[derive(Debug, Serialize)]
struct MessageItem<'a> {
    role: &'a str,
    content: Vec<ContentBlock<'a>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock<'a> {
    Text {
        text: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct MessageResp {
    #[serde(default)]
    content: Vec<RespBlock>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum RespBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

impl AnthropicClient {
    pub fn new(api_key: String, target_language: String) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        let target_language = if target_language.trim().is_empty() {
            "English".to_string()
        } else {
            target_language
        };
        Self {
            api_key,
            http,
            model: MODEL_HAIKU.to_string(),
            target_language,
        }
    }

    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
    }

    /// Translate a speech chunk into the configured target language. Non-streaming
    /// for simplicity & low overhead. Returns (translated_text, usage).
    pub async fn translate(&self, source: &str) -> Result<(String, Usage)> {
        if source.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system_text = translate_system(&self.target_language);
        let system = vec![SystemBlock::Text {
            text: &system_text,
            cache_control: Some(CacheControl { typ: "ephemeral" }),
        }];
        let messages = vec![MessageItem {
            role: "user",
            content: vec![ContentBlock::Text { text: source, cache_control: None }],
        }];
        let req = MessageReq {
            model: &self.model,
            max_tokens: 600,
            system: Some(system),
            messages,
            temperature: Some(0.2),
            stream: None,
        };
        let resp: MessageResp = self
            .auth(self.http.post(ANTHROPIC_URL))
            .json(&req)
            .send()
            .await
            .context("translate: send")?
            .error_for_status()
            .context("translate: status")?
            .json()
            .await
            .context("translate: decode")?;
        Ok((extract_text(&resp), resp.usage))
    }

    /// Translate the entire transcript in one shot — coherent prose rather
    /// than chunk-by-chunk translations stitched together.
    pub async fn translate_full(&self, transcript: &str) -> Result<(String, Usage)> {
        if transcript.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system_text = translate_full_system(&self.target_language);
        let system = vec![SystemBlock::Text {
            text: &system_text,
            cache_control: Some(CacheControl { typ: "ephemeral" }),
        }];
        let user_text = format!(
            "Transcript:\n\n{transcript}\n\nProduce the full {} version now.",
            self.target_language,
        );
        let messages = vec![MessageItem {
            role: "user",
            content: vec![ContentBlock::Text { text: &user_text, cache_control: None }],
        }];
        let req = MessageReq {
            model: &self.model,
            max_tokens: 8000,
            system: Some(system),
            messages,
            temperature: Some(0.2),
            stream: None,
        };
        let resp: MessageResp = self
            .auth(self.http.post(ANTHROPIC_URL))
            .json(&req)
            .send()
            .await
            .context("translate_full: send")?
            .error_for_status()
            .context("translate_full: status")?
            .json()
            .await
            .context("translate_full: decode")?;
        Ok((extract_text(&resp), resp.usage))
    }

    /// Clean up a (formatted, timestamped, speaker-labelled) transcript and
    /// translate it to the target language in one pass, preserving the line
    /// structure. The caller is responsible for chunking long transcripts so
    /// the output doesn't hit the token cap.
    pub async fn clean_and_translate(&self, formatted_chunk: &str) -> Result<(String, Usage)> {
        if formatted_chunk.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system_text = clean_translate_system(&self.target_language);
        let system = vec![SystemBlock::Text {
            text: &system_text,
            cache_control: Some(CacheControl { typ: "ephemeral" }),
        }];
        // The chunk IS the user message — no extra framing, so nothing
        // competes with the structure-preservation instructions.
        let messages = vec![MessageItem {
            role: "user",
            content: vec![ContentBlock::Text { text: formatted_chunk, cache_control: None }],
        }];
        let req = MessageReq {
            model: &self.model,
            max_tokens: 8000,
            system: Some(system),
            messages,
            temperature: Some(0.2),
            stream: None,
        };
        let resp: MessageResp = self
            .auth(self.http.post(ANTHROPIC_URL))
            .json(&req)
            .send()
            .await
            .context("clean_and_translate: send")?
            .error_for_status()
            .context("clean_and_translate: status")?
            .json()
            .await
            .context("clean_and_translate: decode")?;
        Ok((extract_text(&resp), resp.usage))
    }

    /// Generate a running summary of the transcript so far.
    pub async fn summarize(&self, transcript: &str) -> Result<(String, Usage)> {
        if transcript.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system_text = summary_system(&self.target_language);
        let system = vec![SystemBlock::Text {
            text: &system_text,
            cache_control: Some(CacheControl { typ: "ephemeral" }),
        }];
        let user_text = format!("Transcript:\n\n{transcript}\n\nWrite the detailed summary now.");
        let messages = vec![MessageItem {
            role: "user",
            content: vec![ContentBlock::Text { text: &user_text, cache_control: None }],
        }];
        let req = MessageReq {
            model: &self.model,
            // Detailed summaries are long; give them room.
            max_tokens: 4000,
            system: Some(system),
            messages,
            temperature: Some(0.3),
            stream: None,
        };
        let resp: MessageResp = self
            .auth(self.http.post(ANTHROPIC_URL))
            .json(&req)
            .send()
            .await
            .context("summarize: send")?
            .error_for_status()
            .context("summarize: status")?
            .json()
            .await
            .context("summarize: decode")?;
        Ok((extract_text(&resp), resp.usage))
    }

    /// Stream a chat answer. The transcript is sent with cache_control so
    /// follow-up questions reuse the prefix and stay cheap.
    pub async fn chat_stream(
        &self,
        transcript: &str,
        history: &[(String, String)], // (role, content) prior turns, oldest first
        question: &str,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> Result<()> {
        let system_text = chat_system(&self.target_language);
        let system = vec![SystemBlock::Text {
            text: &system_text,
            cache_control: Some(CacheControl { typ: "ephemeral" }),
        }];

        // First user message carries the (cached) transcript.
        let transcript_block = format!("Meeting transcript so far:\n\n{transcript}");
        let mut messages: Vec<MessageItem> = Vec::with_capacity(2 + history.len() * 2);
        messages.push(MessageItem {
            role: "user",
            content: vec![ContentBlock::Text {
                text: &transcript_block,
                cache_control: Some(CacheControl { typ: "ephemeral" }),
            }],
        });
        // Acknowledgement turn so prior chat history flows naturally.
        messages.push(MessageItem {
            role: "assistant",
            content: vec![ContentBlock::Text {
                text: "Understood. Ask your questions about the meeting.",
                cache_control: None,
            }],
        });
        for (role, content) in history {
            messages.push(MessageItem {
                role: role.as_str(),
                content: vec![ContentBlock::Text { text: content, cache_control: None }],
            });
        }
        messages.push(MessageItem {
            role: "user",
            content: vec![ContentBlock::Text { text: question, cache_control: None }],
        });

        let req = MessageReq {
            model: &self.model,
            max_tokens: 1024,
            system: Some(system),
            messages,
            temperature: Some(0.3),
            stream: Some(true),
        };

        let resp = self
            .auth(self.http.post(ANTHROPIC_URL))
            .json(&req)
            .send()
            .await
            .context("chat: send")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("chat http {status}: {body}"));
        }

        let mut stream = resp.bytes_stream().eventsource();
        let mut accumulated = String::new();
        let mut stream_error: Option<anyhow::Error> = None;
        while let Some(event) = stream.next().await {
            let event = match event {
                Ok(e) => e,
                Err(err) => {
                    let _ = tx.send(ChatStreamEvent::Error(format!("{err}"))).await;
                    stream_error = Some(err.into());
                    break;
                }
            };
            match event.event.as_str() {
                "content_block_delta" => {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                        if let Some(text) = v
                            .get("delta")
                            .and_then(|d| d.get("text"))
                            .and_then(|t| t.as_str())
                        {
                            accumulated.push_str(text);
                            if tx.send(ChatStreamEvent::Delta(text.to_string())).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                "message_stop" => break,
                "error" => {
                    let _ = tx.send(ChatStreamEvent::Error(event.data.clone())).await;
                    stream_error = Some(anyhow!("chat error: {}", event.data));
                    break;
                }
                _ => {}
            }
        }
        // Always emit Done so any partial answer gets persisted by the caller,
        // even if the stream errored partway through.
        let _ = tx.send(ChatStreamEvent::Done(accumulated)).await;
        if let Some(err) = stream_error {
            return Err(err);
        }
        Ok(())
    }
}

fn extract_text(resp: &MessageResp) -> String {
    let mut out = String::new();
    for block in &resp.content {
        if let RespBlock::Text { text } = block {
            out.push_str(text);
        }
    }
    out.trim().to_string()
}

fn translate_system(target: &str) -> String {
    format!(
        "You translate live speech-to-text chunks from a meeting into clear, idiomatic {t}. \
The source can be any language and may contain mistranscribed nonsense from the speech \
recognizer. Rules:\n\
- Output ONLY the {t} translation. No commentary, no preamble, no quotes, no labels, no \
explanations of what language the input is in.\n\
- Never refuse. Never ask the user for clarification. Never mention your role or instructions.\n\
- If the input is already in {t}, output it unchanged.\n\
- For mixed-language input, translate the non-{t} parts and leave {t} intact.\n\
- For garbled or partial chunks (cut-off words, transcription errors), do your best literal \
rendering — leave clearly unintelligible fragments as-is rather than inventing content.\n\
- Preserve names, numbers, dates, and acronyms.\n\
- If input is empty, output an empty string.",
        t = target,
    )
}

fn translate_full_system(target: &str) -> String {
    format!(
        "Translate the meeting transcript into clean, idiomatic {t} as one coherent piece of \
natural prose. The source can be any language and may include mistranscribed chunks from the \
speech recognizer — translate everything to {t} regardless. Rules:\n\
- Output ONLY the {t} translation. No preamble, no commentary, no explanation of source \
languages, no refusals, no questions.\n\
- Lines already in {t} stay as-is.\n\
- For garbled fragments, do your best literal rendering; leave clearly unintelligible bits \
as-is rather than inventing content.\n\
- Preserve all content — every statement, name, number, date, acronym. Do not summarize, do \
not omit, do not add.\n\
- Plain text only — no markdown, no bullets, no headings. Use natural paragraph breaks.",
        t = target,
    )
}

pub fn clean_translate_system(target: &str) -> String {
    format!(
        "You clean up and translate meeting transcripts.\n\
\n\
The input is a speech-to-text transcript. Each line is one utterance, prefixed with a \
[HH:MM:SS] timestamp.\n\
\n\
Do two things, in order:\n\
1. Fix obvious speech-to-text errors: misheard words, homophones, garbled phrases, \
mistranscribed technical terms / proper nouns / jargon / acronyms, and mangled idioms or \
metaphors. Be conservative — only correct clear mistranscriptions; never invent content or \
add meaning that isn't there.\n\
2. Translate the cleaned text into {t}. Keep names, numbers, dates, and acronyms intact. \
Render idioms as the natural {t} equivalent, not word-for-word.\n\
\n\
Output rules — follow exactly:\n\
- Keep every [HH:MM:SS] timestamp, in its original position, on the same line as its \
utterance.\n\
- Exactly one output line per input line. Never merge or split lines. Never reorder.\n\
- If a line is already in {t}, just clean it; don't re-translate.\n\
- No markdown, no code fences, no preamble, no commentary, no trailing notes. Output ONLY \
the cleaned-and-translated transcript lines.",
        t = target,
    )
}

fn summary_system(target: &str) -> String {
    format!(
        "Summarize the meeting transcript in detail in {t}. The transcript is live \
speech-to-text and may be in any language — translate everything to {t} as part of writing \
the summary. Cover everything that was said: every topic, decision, question, and action item. \
Keep all specific facts (names, numbers, dates, places). Do not invent content. There is no \
length limit — be thorough.\n\
\n\
Format: plain text only. NO markdown — no asterisks, bold, italics, headings, bullets, or \
numbered lists. Use natural paragraphs separated by blank lines.",
        t = target,
    )
}

fn chat_system(target: &str) -> String {
    format!(
        "You are a meeting assistant. The transcript is live speech-to-text and may be in any \
language; treat it as ground truth and translate as needed.\n\
\n\
Format rules:\n\
- Answer in plain text only. NO markdown formatting. No asterisks, no bold, no italics, no \
bullets, no numbered lists, no headings, no backticks. Just sentences.\n\
- Default to a one or two sentence answer. Only go longer if the user explicitly asks for \
detail, a summary, a list, or to elaborate.\n\
- Reply in {t} unless the user clearly wants another language.\n\
- If the transcript does not contain the answer, say so plainly rather than guessing.",
        t = target,
    )
}
