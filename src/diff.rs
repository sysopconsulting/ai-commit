use anyhow::Result;
use std::fmt;
use std::path::Path;

use crate::git;
use crate::token;

// ---------------------------------------------------------------------------
// DiffMode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffMode {
    Full,
    Compact,
    Stat,
}

impl fmt::Display for DiffMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiffMode::Full => write!(f, "full"),
            DiffMode::Compact => write!(f, "compact"),
            DiffMode::Stat => write!(f, "stat"),
        }
    }
}

impl DiffMode {
    pub fn from_str(s: &str) -> Option<DiffMode> {
        match s {
            "full" => Some(DiffMode::Full),
            "compact" => Some(DiffMode::Compact),
            "stat" => Some(DiffMode::Stat),
            _ => None,
        }
    }
}

/// Get the next smaller diff mode for retry on context-length error.
pub fn next_smaller_mode(mode: DiffMode) -> Option<DiffMode> {
    match mode {
        DiffMode::Full => Some(DiffMode::Compact),
        DiffMode::Compact => Some(DiffMode::Stat),
        DiffMode::Stat => None,
    }
}

// ---------------------------------------------------------------------------
// Core pure logic
// ---------------------------------------------------------------------------

/// Try full → compact → stat in order. If stat still exceeds `max_tokens`,
/// truncate it. Returns the selected text and the mode used.
pub fn select_diff(
    full: &str,
    compact: &str,
    stat: &str,
    max_tokens: usize,
) -> (String, DiffMode) {
    if token::estimate_tokens(full) <= max_tokens {
        return (full.to_string(), DiffMode::Full);
    }
    if token::estimate_tokens(compact) <= max_tokens {
        return (compact.to_string(), DiffMode::Compact);
    }
    if token::estimate_tokens(stat) <= max_tokens {
        return (stat.to_string(), DiffMode::Stat);
    }
    // Stat is still too large — truncate it.
    let truncated = token::truncate_to_tokens(stat, max_tokens);
    (truncated, DiffMode::Stat)
}

// ---------------------------------------------------------------------------
// Git-backed helpers
// ---------------------------------------------------------------------------

/// Fetch the diff for a specific mode from the git index.
pub fn get_forced_diff(repo: &Path, mode: DiffMode) -> Result<String> {
    match mode {
        DiffMode::Full => git::staged_diff(repo, None),
        DiffMode::Compact => git::staged_diff(repo, Some(0)),
        DiffMode::Stat => git::staged_stat(repo),
    }
}

/// Top-level function.
///
/// * If `forced_mode` is not `"auto"`, fetch that specific diff and return it.
/// * Otherwise compute all three variants and call [`select_diff`].
pub fn fit_diff(repo: &Path, max_tokens: usize, forced_mode: &str) -> Result<(String, DiffMode)> {
    if forced_mode != "auto" {
        let mode = DiffMode::from_str(forced_mode)
            .ok_or_else(|| anyhow::anyhow!("unknown diff mode: {}", forced_mode))?;
        let diff = get_forced_diff(repo, mode)?;
        return Ok((diff, mode));
    }

    let full = git::staged_diff(repo, None)?;
    let compact = git::staged_diff(repo, Some(0))?;
    let stat = git::staged_stat(repo)?;

    Ok(select_diff(&full, &compact, &stat, max_tokens))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token;
    use std::fs;
    use std::process::Command;
    use tempfile::tempdir;

    // -----------------------------------------------------------------------
    // Unit tests for next_smaller_mode
    // -----------------------------------------------------------------------

    #[test]
    fn next_smaller_from_full_is_compact() {
        assert_eq!(next_smaller_mode(DiffMode::Full), Some(DiffMode::Compact));
    }

    #[test]
    fn next_smaller_from_compact_is_stat() {
        assert_eq!(next_smaller_mode(DiffMode::Compact), Some(DiffMode::Stat));
    }

    #[test]
    fn next_smaller_from_stat_is_none() {
        assert_eq!(next_smaller_mode(DiffMode::Stat), None);
    }

    // -----------------------------------------------------------------------
    // Unit tests for select_diff (pure, no git)
    // -----------------------------------------------------------------------

    #[test]
    fn select_full_when_fits() {
        let full = "small diff content";
        let compact = "compact";
        let stat = "stat";
        let (text, mode) = select_diff(full, compact, stat, 4096);
        assert_eq!(mode, DiffMode::Full);
        assert_eq!(text, full);
    }

    #[test]
    fn select_compact_when_full_too_large() {
        // full = 10000 bytes, compact = 100 bytes, max = 100 tokens
        let full = "x".repeat(10_000);
        let compact = "y".repeat(100);
        let max_tokens = 100;

        // Verify preconditions
        assert!(token::estimate_tokens(&full) > max_tokens);
        assert!(token::estimate_tokens(&compact) <= max_tokens);

        let (text, mode) = select_diff(&full, &compact, "stat", max_tokens);
        assert_eq!(mode, DiffMode::Compact);
        assert_eq!(text, compact);
    }

    #[test]
    fn select_stat_when_compact_too_large() {
        let full = "x".repeat(100_000);
        let compact = "y".repeat(10_000);
        let stat = "z".repeat(100);
        let max_tokens = 100;

        assert!(token::estimate_tokens(&full) > max_tokens);
        assert!(token::estimate_tokens(&compact) > max_tokens);
        assert!(token::estimate_tokens(&stat) <= max_tokens);

        let (text, mode) = select_diff(&full, &compact, &stat, max_tokens);
        assert_eq!(mode, DiffMode::Stat);
        assert_eq!(text, stat);
    }

    #[test]
    fn select_truncated_stat_when_all_too_large() {
        let full = "x".repeat(100_000);
        let compact = "y".repeat(100_000);
        // Build stat larger than 50 tokens worth of bytes, with newlines so
        // truncate_to_tokens can cut cleanly.
        let stat_line = "some-file.rs | 999 ++++++\n";
        let stat = stat_line.repeat(500); // well over 50 tokens
        let max_tokens = 50;

        assert!(token::estimate_tokens(&full) > max_tokens);
        assert!(token::estimate_tokens(&compact) > max_tokens);
        assert!(token::estimate_tokens(&stat) > max_tokens);

        let (text, mode) = select_diff(&full, &compact, &stat, max_tokens);
        assert_eq!(mode, DiffMode::Stat);
        assert!(
            token::estimate_tokens(&text) <= max_tokens,
            "truncated stat should fit within {} tokens, got {} tokens",
            max_tokens,
            token::estimate_tokens(&text)
        );
        assert!(!text.is_empty());
    }

    // -----------------------------------------------------------------------
    // Integration tests with a real temp repo
    // -----------------------------------------------------------------------

    /// Create an isolated git repo with an initial commit (mirrors git.rs pattern).
    fn init_repo() -> tempfile::TempDir {
        let dir = tempdir().expect("failed to create temp dir");
        let path = dir.path();

        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .output()
            .expect("git init failed");

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(path)
            .output()
            .expect("git config email failed");
        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(path)
            .output()
            .expect("git config name failed");

        fs::write(path.join("README.md"), "# Test\n").expect("write README failed");

        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git add failed");

        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git commit failed");

        dir
    }

    #[test]
    fn fit_diff_with_temp_repo() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("change.txt"), "hello from fit_diff test\n").unwrap();
        Command::new("git")
            .args(["add", "change.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let (diff, mode) = fit_diff(path, 4096, "auto").expect("fit_diff failed");
        assert_eq!(mode, DiffMode::Full, "small change should fit as Full");
        assert!(
            diff.contains("hello from fit_diff test"),
            "diff should contain staged content, got: {diff}"
        );
    }

    #[test]
    fn fit_diff_forced_mode() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("forced.txt"), "forced stat test\n").unwrap();
        Command::new("git")
            .args(["add", "forced.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let (diff, mode) = fit_diff(path, 4096, "stat").expect("fit_diff forced stat failed");
        assert_eq!(mode, DiffMode::Stat);
        assert!(
            diff.contains("forced.txt"),
            "stat output should mention the staged file, got: {diff}"
        );
    }
}
