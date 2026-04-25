use anyhow::Result;
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

pub async fn stream_message(stream: &mut TokenStream) -> Result<String> {
    let mut full = String::new();
    let stdout = io::stdout();
    let mut out = stdout.lock();

    while let Some(token) = stream.next().await {
        let token = token?;
        out.write_all(token.as_bytes())?;
        out.flush()?;
        full.push_str(&token);
    }

    writeln!(out)?;
    out.flush()?;

    Ok(full)
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
    let (cmd, args) = split_editor_command(&editor);
    let tmp = tempfile::NamedTempFile::new()?;
    std::fs::write(tmp.path(), message)?;

    std::process::Command::new(cmd)
        .args(args)
        .arg(tmp.path())
        .status()?;

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
    use super::split_editor_command;

    #[test]
    fn split_editor_command_handles_args() {
        assert_eq!(
            split_editor_command("code --wait"),
            ("code".to_string(), vec!["--wait".to_string()])
        );
    }
}
