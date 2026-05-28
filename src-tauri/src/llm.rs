//! Unified dispatcher across LLM backends. The meeting orchestrator
//! holds an `LlmClient` and never has to branch on which provider is
//! configured — the enum forwards every call to the right concrete
//! client.

use anyhow::Result;
use tokio::sync::mpsc;

use crate::anthropic::{AnthropicClient, ChatStreamEvent, Usage};
use crate::openai::OpenAIClient;
use crate::settings;

pub enum LlmClient {
    Anthropic(AnthropicClient),
    OpenAI(OpenAIClient),
}

impl LlmClient {
    /// Build the right client based on the persisted settings.
    /// `anthropic_key` is the Deepgram-style separately-required key (the
    /// caller already validated it for Anthropic mode); for OpenAI mode
    /// it's ignored.
    pub fn from_settings(
        anthropic_key: String,
        target_language: String,
    ) -> Self {
        match settings::read_llm_provider().as_str() {
            "openai" => {
                let (key, base, model) = settings::read_openai_config();
                LlmClient::OpenAI(OpenAIClient::new(key, base, model, target_language))
            }
            _ => LlmClient::Anthropic(AnthropicClient::new(
                anthropic_key,
                target_language,
            )),
        }
    }

    pub async fn translate(&self, source: &str) -> Result<(String, Usage)> {
        match self {
            LlmClient::Anthropic(c) => c.translate(source).await,
            LlmClient::OpenAI(c) => c.translate(source).await,
        }
    }

    pub async fn translate_full(&self, transcript: &str) -> Result<(String, Usage)> {
        match self {
            LlmClient::Anthropic(c) => c.translate_full(transcript).await,
            LlmClient::OpenAI(c) => c.translate_full(transcript).await,
        }
    }

    pub async fn summarize(&self, transcript: &str) -> Result<(String, Usage)> {
        match self {
            LlmClient::Anthropic(c) => c.summarize(transcript).await,
            LlmClient::OpenAI(c) => c.summarize(transcript).await,
        }
    }

    pub async fn clean_and_translate(&self, formatted_chunk: &str) -> Result<(String, Usage)> {
        match self {
            LlmClient::Anthropic(c) => c.clean_and_translate(formatted_chunk).await,
            LlmClient::OpenAI(c) => c.clean_and_translate(formatted_chunk).await,
        }
    }

    pub async fn chat_stream(
        &self,
        transcript: &str,
        history: &[(String, String)],
        question: &str,
        tx: mpsc::Sender<ChatStreamEvent>,
    ) -> Result<()> {
        match self {
            LlmClient::Anthropic(c) => {
                c.chat_stream(transcript, history, question, tx).await
            }
            LlmClient::OpenAI(c) => {
                c.chat_stream(transcript, history, question, tx).await
            }
        }
    }
}
