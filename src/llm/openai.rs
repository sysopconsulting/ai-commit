use anyhow::{bail, Result};
use super::{Message, TokenStream};
use crate::config::Config;

pub struct OpenAiProvider {
    client: reqwest::Client,
    api_url: String,
    model: String,
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(config: &Config) -> Result<Self> {
        let api_key = config.api_key.clone()
            .ok_or_else(|| anyhow::anyhow!("ACM_API_KEY not set. Run \"acm config set api_key=<key>\""))?;
        let api_url = config.api_url.clone()
            .unwrap_or_else(|| "https://api.openai.com".into());
        Ok(Self { client: reqwest::Client::new(), api_url, model: config.model.clone(), api_key })
    }

    pub async fn chat_stream(&self, _messages: Vec<Message>) -> Result<TokenStream> {
        bail!("OpenAI provider not yet implemented")
    }
}
