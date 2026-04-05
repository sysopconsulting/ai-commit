use anyhow::{Context, Result};
use futures::stream::unfold;
use futures::TryStreamExt;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio_util::io::StreamReader;

use crate::config::Config;
use super::{Message, TokenStream};

pub struct OllamaProvider {
    client: reqwest::Client,
    api_url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(config: &Config) -> Self {
        let api_url = config
            .api_url
            .clone()
            .unwrap_or_else(|| "http://localhost:11434".into());
        OllamaProvider {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            api_url,
            model: config.model.clone(),
        }
    }

    pub async fn chat_stream(&self, messages: Vec<Message>) -> Result<TokenStream> {
        let url = format!("{}/api/chat", self.api_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
        });

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "cannot connect to Ollama at {}. Is it running?",
                    self.api_url
                )
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            let model = self.model.clone();
            anyhow::bail!(
                "model {} not found. Run ollama pull {} or acm config set model=<name>",
                model,
                model
            );
        }
        if !status.is_success() {
            anyhow::bail!("Ollama returned error status: {}", status);
        }

        let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
        let stream_reader = StreamReader::new(byte_stream);
        let buf_reader = BufReader::new(stream_reader);
        let lines: Lines<BufReader<StreamReader<_, _>>> = buf_reader.lines();

        let stream = unfold(lines, |mut lines| async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Some(token) = parse_ollama_line(&line) {
                            return Some((Ok(token), lines));
                        }
                        // Empty content line — skip and continue
                    }
                    Ok(None) => return None,
                    Err(e) => {
                        return Some((Err(anyhow::anyhow!("stream read error: {}", e)), lines))
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

/// Parse a single NDJSON line from Ollama's streaming response.
/// Returns the content token if present and non-empty, otherwise None.
pub fn parse_ollama_line(line: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let content = value.get("message")?.get("content")?.as_str()?;
    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_token() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        assert_eq!(parse_ollama_line(line), Some("Hello".to_string()));
    }

    #[test]
    fn parse_empty_content_returns_none() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true}"#;
        assert_eq!(parse_ollama_line(line), None);
    }

    #[test]
    fn parse_invalid_json_returns_none() {
        assert_eq!(parse_ollama_line("not json"), None);
    }

    #[test]
    fn parse_missing_message_returns_none() {
        let line = r#"{"model":"llama3","done":true}"#;
        assert_eq!(parse_ollama_line(line), None);
    }
}
