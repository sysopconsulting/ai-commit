use anyhow::{Context, Result};
use futures::TryStreamExt;
use futures::stream::unfold;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio_util::io::StreamReader;

use super::{Message, TokenStream};
use crate::config::Config;

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
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".into());
            anyhow::bail!("{}", format_ollama_error(status, &body_text));
        }

        let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
        let stream_reader = StreamReader::new(byte_stream);
        let buf_reader = BufReader::new(stream_reader);
        let lines: Lines<BufReader<StreamReader<_, _>>> = buf_reader.lines();

        let stream = unfold(lines, |mut lines| async move {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        match parse_ollama_line(&line) {
                            Ok(Some(token)) => return Some((Ok(token), lines)),
                            Ok(None) => {}
                            Err(e) => return Some((Err(e), lines)),
                        }
                        // Empty content line — skip and continue
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

/// Parse a single NDJSON line from Ollama's streaming response.
/// Returns the content token if present and non-empty, otherwise None.
pub fn parse_ollama_line(line: &str) -> Result<Option<String>> {
    let value: serde_json::Value = serde_json::from_str(line)
        .with_context(|| format!("invalid Ollama stream JSON: {line}"))?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("Ollama stream error: {}", format_ollama_stream_error(error));
    }
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str());
    match content {
        Some(content) if !content.is_empty() => Ok(Some(content.to_string())),
        _ => Ok(None),
    }
}

fn format_ollama_stream_error(error: &serde_json::Value) -> String {
    error
        .as_str()
        .map(|message| message.to_string())
        .unwrap_or_else(|| error.to_string())
}

fn format_ollama_error(status: reqwest::StatusCode, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("Ollama returned error status: {status}")
    } else {
        format!("Ollama returned {status}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_token() {
        let line =
            r#"{"model":"llama3","message":{"role":"assistant","content":"Hello"},"done":false}"#;
        assert_eq!(parse_ollama_line(line).unwrap(), Some("Hello".to_string()));
    }

    #[test]
    fn parse_empty_content_returns_none() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true}"#;
        assert_eq!(parse_ollama_line(line).unwrap(), None);
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let err = parse_ollama_line("not json").unwrap_err();
        assert!(
            err.to_string().contains("invalid Ollama stream JSON"),
            "error should mention invalid stream JSON, got: {err}"
        );
    }

    #[test]
    fn parse_missing_message_returns_none() {
        let line = r#"{"model":"llama3","done":true}"#;
        assert_eq!(parse_ollama_line(line).unwrap(), None);
    }

    #[test]
    fn parse_error_payload_returns_error() {
        let line = r#"{"error":"context length exceeded"}"#;
        let err = parse_ollama_line(line).unwrap_err();
        assert!(
            err.to_string().contains("context length exceeded"),
            "error should include provider message, got: {err}"
        );
    }

    #[test]
    fn error_message_includes_response_body() {
        let err = format_ollama_error(reqwest::StatusCode::BAD_REQUEST, "context length exceeded");
        assert!(
            err.contains("context length exceeded"),
            "error should include response body, got: {err}"
        );
    }
}
