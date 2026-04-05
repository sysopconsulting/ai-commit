pub mod ollama;
pub mod openai;

use anyhow::Result;
use futures::Stream;
use std::pin::Pin;

use crate::config::Config;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

pub type TokenStream = Pin<Box<dyn Stream<Item = Result<String>> + Send>>;

/// Provider enum — dispatches to Ollama or OpenAI-compatible.
pub enum Provider {
    Ollama(ollama::OllamaProvider),
    OpenAi(openai::OpenAiProvider),
}

impl Provider {
    pub fn from_config(config: &Config) -> Result<Self> {
        match config.provider.as_str() {
            "ollama" => Ok(Provider::Ollama(ollama::OllamaProvider::new(config))),
            "openai" => Ok(Provider::OpenAi(openai::OpenAiProvider::new(config)?)),
            other => anyhow::bail!("unknown provider: {other}. Use \"ollama\" or \"openai\"."),
        }
    }

    pub async fn chat_stream(&self, messages: Vec<Message>) -> Result<TokenStream> {
        match self {
            Provider::Ollama(p) => p.chat_stream(messages).await,
            Provider::OpenAi(p) => p.chat_stream(messages).await,
        }
    }
}
