//! OpenAI-compatible LLM client. Mirrors the surface of `AnthropicClient`
//! so the rest of the meeting orchestrator can swap providers without
//! caring which one is configured.
//!
//! Works against:
//!   * api.openai.com      — gpt-4o-mini, gpt-4o, etc.
//!   * Ollama              — http://localhost:11434/v1, llama3.x, qwen, etc.
//!   * LM Studio           — http://localhost:1234/v1
//!   * vLLM                — http://localhost:8000/v1
//!   * OpenRouter, Together, Groq, Cerebras, llama.cpp's server… anything
//!     that speaks the `/chat/completions` shape.
//!
//! No prompt caching (only Anthropic offers that today), so the Usage
//! struct's cache fields stay zero. Input/output tokens are tracked
//! normally and accumulated into the meeting cost just like Anthropic
//! tokens — the cost-per-token used by the TopBar's $ estimate will be
//! wrong for non-OpenAI backends, but the token counts themselves are
//! accurate and the user can read them in the meeting's cost block.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::anthropic::{ChatStreamEvent, Usage};

pub struct OpenAIClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
    model: String,
    target_language: String,
}

#[derive(Debug, Serialize)]
struct ChatReq<'a> {
    model: &'a str,
    messages: Vec<ChatMsg<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ChatMsg<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize, Default)]
struct ChatResp {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: OpenAIUsage,
}

#[derive(Debug, Deserialize)]
struct Choice {
    #[serde(default)]
    message: ChoiceMsg,
}

#[derive(Debug, Default, Deserialize)]
struct ChoiceMsg {
    #[serde(default)]
    content: String,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAIUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

fn to_usage(u: OpenAIUsage) -> Usage {
    Usage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: 0,
    }
}

impl OpenAIClient {
    pub fn new(
        api_key: String,
        base_url: String,
        model: String,
        target_language: String,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .pool_idle_timeout(Duration::from_secs(60))
            .build()
            .expect("reqwest client");
        let target_language = if target_language.trim().is_empty() {
            "English".to_string()
        } else {
            target_language
        };
        let base_url = if base_url.trim().is_empty() {
            "https://api.openai.com/v1".to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        let model = if model.trim().is_empty() {
            "gpt-4o-mini".to_string()
        } else {
            model
        };
        Self {
            api_key,
            base_url,
            http,
            model,
            target_language,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let b = builder.header("content-type", "application/json");
        if self.api_key.is_empty() {
            b
        } else {
            b.header("authorization", format!("Bearer {}", self.api_key))
        }
    }

    async fn single(
        &self,
        system: &str,
        user: &str,
        max_tokens: Option<u32>,
        temperature: Option<f32>,
    ) -> Result<(String, Usage)> {
        let req = ChatReq {
            model: &self.model,
            messages: vec![
                ChatMsg { role: "system", content: system },
                ChatMsg { role: "user", content: user },
            ],
            max_tokens,
            temperature,
            stream: None,
        };
        let resp = self
            .auth(self.http.post(self.endpoint()))
            .json(&req)
            .send()
            .await
            .context("openai: send")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("openai http {status}: {body}"));
        }
        let body: ChatResp = resp.json().await.context("openai: decode")?;
        let text = body
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default()
            .trim()
            .to_string();
        Ok((text, to_usage(body.usage)))
    }

    pub async fn translate(&self, source: &str) -> Result<(String, Usage)> {
        if source.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system = translate_system(&self.target_language);
        self.single(&system, source, Some(600), Some(0.2)).await
    }

    pub async fn translate_full(&self, transcript: &str) -> Result<(String, Usage)> {
        if transcript.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system = translate_full_system(&self.target_language);
        let user = format!(
            "Transcript:\n\n{transcript}\n\nProduce the full {} version now.",
            self.target_language,
        );
        self.single(&system, &user, Some(8000), Some(0.2)).await
    }

    pub async fn summarize(&self, transcript: &str) -> Result<(String, Usage)> {
        if transcript.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system = summary_system(&self.target_language);
        let user = format!("Transcript:\n\n{transcript}\n\nWrite the detailed summary now.");
        self.single(&system, &user, Some(4000), Some(0.3)).await
    }

    /// Clean up + translate a formatted transcript chunk, preserving the
    /// timestamp + speaker line structure. Mirrors AnthropicClient's method.
    pub async fn clean_and_translate(&self, formatted_chunk: &str) -> Result<(String, Usage)> {
        if formatted_chunk.trim().is_empty() {
            return Ok((String::new(), Usage::default()));
        }
        let system = crate::anthropic::clean_translate_system(&self.target_language);
        self.single(&system, formatted_chunk, Some(8000), Some(0.2)).await
    }

    /// Stream a chat answer. Same shape as AnthropicClient::chat_stream so
    /// the orchestrator doesn't have to care which backend it's talking to.
    pub async fn chat_stream(
        &self,
        transcript: &str,
        history: &[(String, String)],
        question: &str,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> Result<()> {
        let system = chat_system(&self.target_language);
        let transcript_block =
            format!("Meeting transcript so far:\n\n{transcript}");

        let mut messages: Vec<ChatMsg> = Vec::with_capacity(4 + history.len());
        messages.push(ChatMsg { role: "system", content: &system });
        messages.push(ChatMsg { role: "user", content: &transcript_block });
        messages.push(ChatMsg {
            role: "assistant",
            content: "Understood. Ask your questions about the meeting.",
        });
        for (role, content) in history {
            messages.push(ChatMsg { role: role.as_str(), content });
        }
        messages.push(ChatMsg { role: "user", content: question });

        let req = ChatReq {
            model: &self.model,
            messages,
            max_tokens: Some(1024),
            temperature: Some(0.3),
            stream: Some(true),
        };

        let resp = self
            .auth(self.http.post(self.endpoint()))
            .json(&req)
            .send()
            .await
            .context("openai chat: send")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("openai chat http {status}: {body}"));
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
            // OpenAI streams send `data: [DONE]` as the terminator.
            if event.data.trim() == "[DONE]" {
                break;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&event.data) {
                if let Some(delta) = v
                    .get("choices")
                    .and_then(|c| c.get(0))
                    .and_then(|c| c.get("delta"))
                    .and_then(|d| d.get("content"))
                    .and_then(|t| t.as_str())
                {
                    if !delta.is_empty() {
                        accumulated.push_str(delta);
                        if tx
                            .send(ChatStreamEvent::Delta(delta.to_string()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }
        let _ = tx.send(ChatStreamEvent::Done(accumulated)).await;
        if let Some(err) = stream_error {
            return Err(err);
        }
        Ok(())
    }
}

// --- Prompts. Duplicated from anthropic.rs so OpenAI mode is fully
// self-contained; they're cheap string formats, no point refactoring
// into a shared module today.

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
