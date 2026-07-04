use anyhow::{Result, bail};
use crossterm::event::{Event, KeyCode, KeyModifiers, read};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use std::io::{self, IsTerminal, Write};

use crate::llm::TokenStream;

#[derive(Debug, PartialEq)]
pub enum Action {
    Commit,
    Edit,
    Regenerate,
    Cancel,
}

struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub fn print_summary(files: usize, insertions: usize, deletions: usize) {
    let file_word = if files == 1 { "file" } else { "files" };
    if insertions > 0 || deletions > 0 {
        eprintln!(
            "  Staged: {} {} (+{}, -{})",
            files, file_word, insertions, deletions
        );
    } else {
        eprintln!("  Staged: {} {}", files, file_word);
    }
}

/// A streamed provider response: `raw` is everything the model sent,
/// `displayed` is what the user actually saw (reasoning spans filtered).
pub struct StreamedMessage {
    pub raw: String,
    pub displayed: String,
}

/// Stateful display filter that suppresses `<think>...</think>` spans in a
/// token stream. Tags split across chunk boundaries are held in `pending`
/// until enough bytes arrive to decide.
struct ThinkFilter {
    depth: usize,
    placeholder_emitted: bool,
    pending: String,
}

/// A partial tag candidate longer than this is flushed as literal text.
const MAX_PENDING_TAG: usize = 64;

impl ThinkFilter {
    fn new() -> Self {
        Self {
            depth: 0,
            placeholder_emitted: false,
            pending: String::new(),
        }
    }

    /// Feed a chunk; returns (visible text, whether suppression just started
    /// for the first time — the caller may show a placeholder).
    fn push(&mut self, chunk: &str) -> (String, bool) {
        let mut buf = std::mem::take(&mut self.pending);
        buf.push_str(chunk);
        let mut visible = String::new();
        let mut placeholder = false;
        let mut i = 0;

        while i < buf.len() {
            let Some(off) = buf[i..].find('<') else {
                if self.depth == 0 {
                    visible.push_str(&buf[i..]);
                }
                break;
            };
            let pos = i + off;
            if self.depth == 0 {
                visible.push_str(&buf[i..pos]);
            }
            if let Some((len, closing)) = crate::prompt::think_tag_at(&buf[pos..]) {
                if closing {
                    self.depth = self.depth.saturating_sub(1);
                } else {
                    if self.depth == 0 && !self.placeholder_emitted {
                        placeholder = true;
                        self.placeholder_emitted = true;
                    }
                    self.depth += 1;
                }
                i = pos + len;
            } else if buf.len() - pos < MAX_PENDING_TAG
                && crate::prompt::could_be_think_tag_prefix(&buf[pos..])
            {
                // Might be a tag split across chunks — hold and decide later.
                self.pending = buf[pos..].to_string();
                return (visible, placeholder);
            } else {
                if self.depth == 0 {
                    visible.push('<');
                }
                i = pos + 1;
            }
        }
        (visible, placeholder)
    }

    /// Stream ended: any held partial tag is literal text (unless we are
    /// inside a think span, where it is reasoning).
    fn flush(&mut self) -> String {
        let pending = std::mem::take(&mut self.pending);
        if self.depth == 0 { pending } else { String::new() }
    }
}

pub async fn stream_message(
    stream: &mut TokenStream,
    show_thinking_placeholder: bool,
) -> Result<StreamedMessage> {
    let mut raw = String::new();
    let mut displayed = String::new();
    let mut filter = ThinkFilter::new();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    while let Some(token) = stream.next().await {
        let token = token?;
        raw.push_str(&token);
        let (visible, thinking_started) = filter.push(&token);
        if thinking_started && show_thinking_placeholder {
            eprintln!("  (thinking…)");
        }
        if !visible.is_empty() {
            out.write_all(visible.as_bytes())?;
            out.flush()?;
            displayed.push_str(&visible);
        }
    }
    let tail = filter.flush();
    if !tail.is_empty() {
        out.write_all(tail.as_bytes())?;
        displayed.push_str(&tail);
    }

    writeln!(out)?;
    out.flush()?;

    Ok(StreamedMessage { raw, displayed })
}

pub fn prompt_action() -> Result<Action> {
    eprint!("  [y]es  [e]dit  [r]egenerate  [n]o  > ");
    io::stderr().flush()?;

    let raw_mode = RawModeGuard::enable()?;
    let action = loop {
        if let Event::Key(key) = read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break Action::Cancel,
                (KeyCode::Char('y'), _) => break Action::Commit,
                (KeyCode::Enter, _) => break Action::Commit,
                (KeyCode::Char('e'), _) => break Action::Edit,
                (KeyCode::Char('r'), _) => break Action::Regenerate,
                (KeyCode::Char('n'), _) => break Action::Cancel,
                (KeyCode::Esc, _) => break Action::Cancel,
                _ => {}
            }
        }
    };
    drop(raw_mode);
    eprintln!();

    Ok(action)
}

/// Ask user whether to stage all changes. Returns true if yes.
/// Returns false silently if not running in a TTY.
pub fn prompt_stage() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("  Stage all changes? [y/n] > ");
    io::stderr().flush()?;

    let raw_mode = RawModeGuard::enable()?;
    let yes = loop {
        if let Event::Key(key) = read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break false,
                (KeyCode::Char('y'), _) | (KeyCode::Enter, _) => break true,
                (KeyCode::Char('n'), _) | (KeyCode::Esc, _) => break false,
                _ => {}
            }
        }
    };
    drop(raw_mode);
    eprintln!();

    Ok(yes)
}

/// Ask user whether to push after commit. Returns true if yes.
/// Returns false silently if not running in a TTY.
pub fn prompt_push() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    eprint!("  Push? [y/n] > ");
    io::stderr().flush()?;

    let raw_mode = RawModeGuard::enable()?;
    let yes = loop {
        if let Event::Key(key) = read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => break false,
                (KeyCode::Char('y'), _) => break true,
                (KeyCode::Char('n'), _) | (KeyCode::Enter, _) | (KeyCode::Esc, _) => break false,
                _ => {}
            }
        }
    };
    drop(raw_mode);
    eprintln!();

    Ok(yes)
}

/// Print `--stat` output for files about to be staged, indented.
pub fn print_unstaged_stat(stat: &str) {
    if stat.is_empty() {
        return;
    }
    eprintln!();
    for line in stat.lines() {
        eprintln!("  {line}");
    }
    eprintln!();
}

pub fn edit_message(message: &str) -> Result<String> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".into());
    edit_message_with_editor(&editor, message)
}

fn edit_message_with_editor(editor: &str, message: &str) -> Result<String> {
    let (cmd, args) = split_editor_command(editor);
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), message)?;

    let status = std::process::Command::new(cmd)
        .args(args)
        .arg(tmp.path())
        .status()?;
    if !status.success() {
        bail!("editor exited with status: {status}");
    }

    let content = std::fs::read_to_string(tmp.path())?;
    Ok(content.trim().to_string())
}

fn split_editor_command(editor: &str) -> (String, Vec<String>) {
    let mut parts = editor.split_whitespace();
    let cmd = parts.next().unwrap_or("vi").to_string();
    let args = parts.map(|part| part.to_string()).collect();
    (cmd, args)
}

#[cfg(test)]
mod tests {
    use super::{ThinkFilter, edit_message_with_editor, split_editor_command};

    /// Feed chunks through a ThinkFilter and collect the displayed text and
    /// whether a placeholder was requested.
    fn run_filter(chunks: &[&str]) -> (String, bool) {
        let mut filter = ThinkFilter::new();
        let mut displayed = String::new();
        let mut placeholder = false;
        for chunk in chunks {
            let (visible, p) = filter.push(chunk);
            displayed.push_str(&visible);
            placeholder |= p;
        }
        displayed.push_str(&filter.flush());
        (displayed, placeholder)
    }

    #[test]
    fn filter_passes_plain_text_through() {
        let (out, placeholder) = run_filter(&["feat: add ", "login endpoint"]);
        assert_eq!(out, "feat: add login endpoint");
        assert!(!placeholder);
    }

    #[test]
    fn filter_suppresses_think_span_and_requests_placeholder() {
        let (out, placeholder) =
            run_filter(&["<think>internal reasoning</think>", "fix: handle empty input"]);
        assert_eq!(out, "fix: handle empty input");
        assert!(placeholder);
    }

    #[test]
    fn filter_handles_tag_split_across_chunks() {
        let (out, placeholder) = run_filter(&[
            "<th", "ink>reason", "ing</th", "ink>", "feat: add retry",
        ]);
        assert_eq!(out, "feat: add retry");
        assert!(placeholder);
    }

    #[test]
    fn filter_keeps_non_think_angle_brackets() {
        let (out, _) = run_filter(&["feat: support <T> generics ", "<div> too"]);
        assert_eq!(out, "feat: support <T> generics <div> too");
    }

    #[test]
    fn filter_unclosed_think_suppresses_to_end() {
        let (out, placeholder) = run_filter(&["<think>never closes ", "more reasoning"]);
        assert_eq!(out, "");
        assert!(placeholder);
    }

    #[test]
    fn filter_flushes_trailing_partial_non_tag() {
        let (out, _) = run_filter(&["fix: compare a ", "<themes"]);
        assert_eq!(out, "fix: compare a <themes");
    }

    #[test]
    fn split_editor_command_handles_args() {
        assert_eq!(
            split_editor_command("code --wait"),
            ("code".to_string(), vec!["--wait".to_string()])
        );
    }

    #[test]
    fn edit_message_with_editor_returns_error_when_editor_fails() {
        let err = edit_message_with_editor("false", "message").unwrap_err();
        assert!(
            err.to_string().contains("editor exited with status"),
            "Expected editor status error, got: {err}"
        );
    }
}
