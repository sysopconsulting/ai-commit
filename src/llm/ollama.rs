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

    /// Build the request body. `think` is omitted entirely for the
    /// compatibility retry — older Ollama versions reject the field for models
    /// without thinking support.
    fn request_body(&self, messages: &[Message], with_think: bool) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "options": {
                // Near-deterministic: this task has one right answer.
                "temperature": TEMPERATURE,
                // Bound runaway generation. A 6-bullet message is ~120 tokens.
                "num_predict": NUM_PREDICT,
            },
        });
        if with_think {
            // Suppress reasoning tokens; acm discards them anyway.
            body["think"] = serde_json::Value::Bool(false);
        }
        body
    }

    pub async fn chat_stream(&self, messages: Vec<Message>) -> Result<TokenStream> {
        let url = format!("{}/api/chat", self.api_url);

        let mut response = self
            .client
            .post(&url)
            .json(&self.request_body(&messages, true))
            .send()
            .await
            .with_context(|| {
                format!(
                    "cannot connect to Ollama at {}. Is it running?",
                    self.api_url
                )
            })?;

        // A missing model must keep its own diagnostic and is never retried.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            let model = self.model.clone();
            anyhow::bail!(
                "model {} not found. Run ollama pull {} or acm config set model=<name>",
                model,
                model
            );
        }

        // Best-effort compatibility: a server that rejects `think` as an
        // unknown field gets one retry without it. Not a tested guarantee.
        if is_request_validation_status(response.status()) {
            let status = response.status();
            let first_body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".into());
            if first_body.to_lowercase().contains("think") {
                response = self
                    .client
                    .post(&url)
                    .json(&self.request_body(&messages, false))
                    .send()
                    .await
                    .with_context(|| format!("cannot connect to Ollama at {}", self.api_url))?;
                if !response.status().is_success() {
                    let second_status = response.status();
                    let second_body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unreadable body>".into());
                    anyhow::bail!(
                        "{} (retry without \"think\" also failed; first attempt was {})",
                        format_ollama_error(second_status, &second_body),
                        status
                    );
                }
            } else {
                anyhow::bail!("{}", format_ollama_error(status, &first_body));
            }
        }

        let status = response.status();
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

        let stream = unfold(StreamState::new(lines), |mut st| async move {
            // A terminal error queued alongside the final content chunk is
            // emitted on the next poll, then the stream ends.
            if let Some(err) = st.pending_error.take() {
                st.finished = true;
                return Some((Err(err), st));
            }
            if st.finished {
                return None;
            }
            loop {
                match st.lines.next_line().await {
                    Ok(Some(line)) => {
                        let chunk = match parse_ollama_line(&line) {
                            Ok(c) => c,
                            Err(e) => {
                                st.finished = true;
                                return Some((Err(e), st));
                            }
                        };
                        if chunk.tool_call {
                            st.tool_call_seen = true;
                        }
                        // Whitespace-only content must not count as real
                        // output, or a tool call preceded by " \n" would report
                        // the misleading "empty message" error instead.
                        let content = chunk.content.clone().filter(|c| !c.is_empty());
                        if content.as_deref().is_some_and(|c| !c.trim().is_empty()) {
                            st.content_seen = true;
                        }
                        let terminal = chunk.done.then(|| st.terminal_error(&chunk)).flatten();
                        match (content, terminal) {
                            (Some(c), Some(err)) => {
                                st.pending_error = Some(err);
                                return Some((Ok(c), st));
                            }
                            (Some(c), None) => return Some((Ok(c), st)),
                            (None, Some(err)) => {
                                st.finished = true;
                                return Some((Err(err), st));
                            }
                            (None, None) => {}
                        }
                        if chunk.done {
                            return None;
                        }
                    }
                    // EOF without a `done` record: apply the same checks.
                    Ok(None) => {
                        st.finished = true;
                        return st.eof_error().map(|e| (Err(e), st));
                    }
                    Err(e) => {
                        st.finished = true;
                        return Some((Err(anyhow::anyhow!("stream read error: {}", e)), st));
                    }
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

/// Sampling settings sent with every Ollama request.
const TEMPERATURE: f32 = 0.2;
const NUM_PREDICT: u32 = 512;

fn is_request_validation_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::BAD_REQUEST
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
}

/// One parsed NDJSON record. A single record can carry content *and* terminal
/// metadata at once, so this is a record rather than an enum.
#[derive(Debug, Default, PartialEq)]
pub struct Chunk {
    pub content: Option<String>,
    pub done: bool,
    pub done_reason: Option<String>,
    pub tool_call: bool,
}

struct StreamState<R> {
    lines: Lines<R>,
    content_seen: bool,
    tool_call_seen: bool,
    pending_error: Option<anyhow::Error>,
    finished: bool,
}

impl<R> StreamState<R> {
    fn new(lines: Lines<R>) -> Self {
        Self {
            lines,
            content_seen: false,
            tool_call_seen: false,
            pending_error: None,
            finished: false,
        }
    }

    /// The error to raise once the provider says it is done, if any.
    fn terminal_error(&self, chunk: &Chunk) -> Option<anyhow::Error> {
        if chunk.done_reason.as_deref() == Some("length") {
            return Some(anyhow::anyhow!(
                "model output hit the {NUM_PREDICT}-token cap and is truncated. \
                 Re-run, or use a model that follows the format more closely."
            ));
        }
        self.no_content_error()
    }

    fn eof_error(&self) -> Option<anyhow::Error> {
        self.no_content_error()
    }

    fn no_content_error(&self) -> Option<anyhow::Error> {
        (self.tool_call_seen && !self.content_seen).then(|| {
            anyhow::anyhow!(
                "model returned a tool call instead of a commit message. \
                 This model is not usable with acm — try another (e.g. a coder model)."
            )
        })
    }
}

/// Parse a single NDJSON line from Ollama's streaming response.
///
/// One record may carry content, `done`, `done_reason` and `tool_calls`
/// simultaneously, so every field is reported and the caller decides.
pub fn parse_ollama_line(line: &str) -> Result<Chunk> {
    let value: serde_json::Value = serde_json::from_str(line)
        .with_context(|| format!("invalid Ollama stream JSON: {line}"))?;
    if let Some(error) = value.get("error") {
        anyhow::bail!("Ollama stream error: {}", format_ollama_stream_error(error));
    }
    let message = value.get("message");
    Ok(Chunk {
        content: message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .map(str::to_string),
        done: value.get("done").and_then(|d| d.as_bool()).unwrap_or(false),
        done_reason: value
            .get("done_reason")
            .and_then(|d| d.as_str())
            .map(str::to_string),
        tool_call: message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|t| t.as_array())
            .is_some_and(|calls| !calls.is_empty()),
    })
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
        let chunk = parse_ollama_line(line).unwrap();
        assert_eq!(chunk.content.as_deref(), Some("Hello"));
        assert!(!chunk.done);
        assert!(!chunk.tool_call);
    }

    #[test]
    fn parse_empty_content_returns_none() {
        let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true}"#;
        let chunk = parse_ollama_line(line).unwrap();
        assert_eq!(chunk.content, None);
        assert!(chunk.done);
    }

    #[test]
    fn parse_reports_tool_calls() {
        let line = r#"{"message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"open_file"}}]},"done":true,"done_reason":"stop"}"#;
        let chunk = parse_ollama_line(line).unwrap();
        assert!(chunk.tool_call, "tool_calls must be reported");
        assert_eq!(chunk.content, None);
        assert!(chunk.done);
    }

    #[test]
    fn parse_reports_content_and_terminal_metadata_together() {
        // One record can carry the final content fragment AND the stop reason;
        // neither may be lost.
        let line = r#"{"message":{"role":"assistant","content":"tail"},"done":true,"done_reason":"length"}"#;
        let chunk = parse_ollama_line(line).unwrap();
        assert_eq!(chunk.content.as_deref(), Some("tail"));
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("length"));
    }

    #[test]
    fn parse_empty_tool_calls_array_is_not_a_tool_call() {
        let line = r#"{"message":{"role":"assistant","content":"x","tool_calls":[]},"done":false}"#;
        assert!(!parse_ollama_line(line).unwrap().tool_call);
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
        assert_eq!(parse_ollama_line(line).unwrap().content, None);
    }

    #[test]
    fn request_body_carries_options_and_think() {
        let cfg = Config {
            model: "qwen3-coder:30b".into(),
            ..Config::default()
        };
        let p = OllamaProvider::new(&cfg);
        let msgs = vec![Message {
            role: super::super::Role::User,
            content: "hi".into(),
        }];

        let with = p.request_body(&msgs, true);
        assert_eq!(with["think"], serde_json::json!(false));
        assert_eq!(
            with["options"]["num_predict"],
            serde_json::json!(NUM_PREDICT)
        );
        assert_eq!(with["stream"], serde_json::json!(true));
        assert!(
            (with["options"]["temperature"].as_f64().unwrap() - TEMPERATURE as f64).abs() < 1e-6
        );

        // The retry drops only `think`; options must survive.
        let without = p.request_body(&msgs, false);
        assert!(
            without.get("think").is_none(),
            "retry body must omit think entirely"
        );
        assert_eq!(
            without["options"]["num_predict"],
            serde_json::json!(NUM_PREDICT)
        );
    }

    #[test]
    fn only_request_validation_statuses_trigger_the_think_retry() {
        use reqwest::StatusCode;
        assert!(is_request_validation_status(StatusCode::BAD_REQUEST));
        assert!(is_request_validation_status(
            StatusCode::UNPROCESSABLE_ENTITY
        ));
        // 404 keeps its own model-not-found diagnostic and must not be retried.
        assert!(!is_request_validation_status(StatusCode::NOT_FOUND));
        assert!(!is_request_validation_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
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
