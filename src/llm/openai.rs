use anyhow::{Context, Result};
use futures::TryStreamExt;
use futures::stream::unfold;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio_util::io::StreamReader;

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
        let api_key = config.api_key.clone().ok_or_else(|| {
            anyhow::anyhow!("ACM_API_KEY not set. Run \"acm config set api_key=<key>\"")
        })?;
        let api_url = config
            .api_url
            .clone()
            .unwrap_or_else(|| "https://api.openai.com".into());
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            api_url,
            model: config.model.clone(),
            api_key,
        })
    }

    pub async fn chat_stream(&self, messages: Vec<Message>) -> Result<TokenStream> {
        let url = format!("{}/v1/chat/completions", self.api_url);
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("cannot connect to API at {}", self.api_url))?;

        let status = response.status();
        if !status.is_success() {
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".into());
            anyhow::bail!("API returned {}: {}", status, body_text);
        }

        let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
        let stream_reader = StreamReader::new(byte_stream);
        let buf_reader = BufReader::new(stream_reader);
        let lines: Lines<BufReader<StreamReader<_, _>>> = buf_reader.lines();

        let stream = unfold(lines, |mut lines| async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        match parse_openai_line(&line) {
                            Ok(Some(token)) => return Some((Ok(token), lines)),
                            Ok(None) => {}
                            Err(e) => return Some((Err(e), lines)),
                        }
                        // Empty content or non-data line — skip and continue
                    }
                    Ok(None) => return None,
                    Err(e) => {
                        return Some((Err(anyhow::anyhow!("stream read error: {}", e)), lines));
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

/// Parse a single SSE line from OpenAI's streaming response.
/// Lines are prefixed with "data: ".
/// Returns the content token if present and non-empty, otherwise None.
pub fn parse_openai_line(line: &str) -> Result<Option<String>> {
    let Some(json_str) = line.strip_prefix("data: ") else {
        return Ok(None);
    };
    if json_str == "[DONE]" {
        return Ok(None);
    }
    let value: serde_json::Value = serde_json::from_str(json_str)
        .with_context(|| format!("invalid OpenAI stream JSON: {json_str}"))?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("OpenAI stream error: {}", format_openai_error(error));
    }
    let content = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(|content| content.as_str());
    match content {
        Some(content) if !content.is_empty() => Ok(Some(content.to_string())),
        _ => Ok(None),
    }
}

fn format_openai_error(error: &serde_json::Value) -> String {
    error
        .get("message")
        .and_then(|message| message.as_str())
        .map(|message| message.to_string())
        .unwrap_or_else(|| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_token() {
        let line = r#"data: {"id":"x","object":"chat.completion.chunk","choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
        assert_eq!(parse_openai_line(line).unwrap(), Some("Hello".to_string()));
    }

    #[test]
    fn parse_done_returns_none() {
        assert_eq!(parse_openai_line("data: [DONE]").unwrap(), None);
    }

    #[test]
    fn parse_empty_line_returns_none() {
        assert_eq!(parse_openai_line("").unwrap(), None);
    }

    #[test]
    fn parse_no_data_prefix_returns_none() {
        assert_eq!(parse_openai_line("event: message").unwrap(), None);
    }

    #[test]
    fn parse_delta_without_content_returns_none() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"role":"assistant"},"index":0}]}"#;
        assert_eq!(parse_openai_line(line).unwrap(), None);
    }

    #[test]
    fn parse_empty_content_returns_none() {
        let line = r#"data: {"id":"x","choices":[{"delta":{"content":""},"index":0}]}"#;
        assert_eq!(parse_openai_line(line).unwrap(), None);
    }

    #[test]
    fn parse_error_payload_returns_error() {
        let line = r#"data: {"error":{"message":"context length exceeded"}}"#;
        let err = parse_openai_line(line).unwrap_err();
        assert!(
            err.to_string().contains("context length exceeded"),
            "error should include provider message, got: {err}"
        );
    }
}
