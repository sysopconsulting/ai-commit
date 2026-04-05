use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Run `git` with `args` in `dir`. Returns trimmed stdout on success.
/// On failure, returns an error containing stderr.
fn git_in(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git {}: {}", args.join(" "), stderr)
    }
}

/// Returns the list of staged files. Errors if no files are staged.
pub fn staged_files(repo: &Path) -> Result<Vec<String>> {
    let output = git_in(repo, &["diff", "--cached", "--name-only"])?;
    if output.is_empty() {
        bail!("no staged changes. Stage files with \"git add\" first.")
    }
    Ok(output.lines().map(|l| l.to_string()).collect())
}

/// Returns the full staged diff. Pass `context_lines = Some(0)` for a compact diff.
/// `None` uses git's default (3 context lines).
pub fn staged_diff(repo: &Path, context_lines: Option<u32>) -> Result<String> {
    let mut args = vec!["diff", "--cached"];
    let unified_arg;
    if let Some(n) = context_lines {
        unified_arg = format!("--unified={}", n);
        args.push(&unified_arg);
    }
    git_in(repo, &args)
}

/// Returns `git diff --cached --stat` output.
pub fn staged_stat(repo: &Path) -> Result<String> {
    git_in(repo, &["diff", "--cached", "--stat"])
}

/// Returns `(file_count, total_insertions, total_deletions)` from `git diff --cached --numstat`.
pub fn staged_summary(repo: &Path) -> Result<(usize, usize, usize)> {
    let output = git_in(repo, &["diff", "--cached", "--numstat"])?;
    let mut files = 0usize;
    let mut insertions = 0usize;
    let mut deletions = 0usize;

    for line in output.lines() {
        // Each line: "<added>\t<removed>\t<filename>"
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() < 2 {
            continue;
        }
        // Binary files show '-' instead of a number
        let added: usize = parts[0].parse().unwrap_or(0);
        let removed: usize = parts[1].parse().unwrap_or(0);
        insertions += added;
        deletions += removed;
        files += 1;
    }

    Ok((files, insertions, deletions))
}

/// Returns true if there are unstaged or untracked changes in the working tree.
pub fn has_unstaged_changes(repo: &Path) -> bool {
    // Check for modified/deleted tracked files + untracked files
    git_in(repo, &["status", "--porcelain"])
        .map(|out| !out.is_empty())
        .unwrap_or(false)
}

/// Stage all changes (tracked + untracked) in the repo.
pub fn stage_all(repo: &Path) -> Result<()> {
    git_in(repo, &["add", "-A"])?;
    Ok(())
}

/// Runs `git commit -m <message>` in `repo`.
pub fn commit(repo: &Path, message: &str) -> Result<()> {
    git_in(repo, &["commit", "-m", message])?;
    Ok(())
}

/// Push the current branch to origin.
pub fn push(repo: &Path) -> Result<()> {
    git_in(repo, &["push"])?;
    Ok(())
}

/// Returns the root of the current git repository.
pub fn repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to run: git rev-parse --show-toplevel")?;

    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(path))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git rev-parse --show-toplevel: {}", stderr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{tempdir, TempDir};

    /// Create an isolated git repo with an initial commit.
    fn init_repo() -> TempDir {
        let dir = tempdir().expect("failed to create temp dir");
        let path = dir.path();

        // git init -b main
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .expect("git init failed");

        // Configure user locally so commits work
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

        // Write README.md
        fs::write(path.join("README.md"), "# Test\n").expect("write README failed");

        // git add + git commit
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
    fn no_staged_files_returns_error() {
        let repo = init_repo();
        let err = staged_files(repo.path()).unwrap_err();
        assert!(
            err.to_string().contains("no staged changes"),
            "Expected 'no staged changes', got: {}",
            err
        );
    }

    #[test]
    fn staged_files_lists_files() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("a.txt"), "file a\n").unwrap();
        fs::write(path.join("b.txt"), "file b\n").unwrap();

        Command::new("git")
            .args(["add", "a.txt", "b.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let files = staged_files(path).unwrap();
        assert!(files.contains(&"a.txt".to_string()));
        assert!(files.contains(&"b.txt".to_string()));
    }

    #[test]
    fn staged_diff_returns_diff() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("new_file.txt"), "hello world\n").unwrap();
        Command::new("git")
            .args(["add", "new_file.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let diff = staged_diff(path, None).unwrap();
        assert!(
            diff.contains("hello world"),
            "Diff should contain file content, got: {diff}"
        );
    }

    #[test]
    fn staged_diff_compact_has_no_context_lines() {
        let repo = init_repo();
        let path = repo.path();

        // Write a 5-line file and commit it
        let original = "line1\nline2\nline3\nline4\nline5\n";
        fs::write(path.join("five.txt"), original).unwrap();
        Command::new("git")
            .args(["add", "five.txt"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add five.txt"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();

        // Modify the middle line (line3)
        let modified = "line1\nline2\nLINE3_CHANGED\nline4\nline5\n";
        fs::write(path.join("five.txt"), modified).unwrap();
        Command::new("git")
            .args(["add", "five.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        // Compact diff (context=0) should NOT contain surrounding context lines.
        // Context lines in a diff start with a space character. The hunk header (@@...@@)
        // may echo the nearby function/context label, so we check for lines starting with ' '.
        let compact = staged_diff(path, Some(0)).unwrap();

        let context_lines_present: Vec<&str> = compact
            .lines()
            .filter(|l| l.starts_with(' '))
            .collect();
        assert!(
            context_lines_present.is_empty(),
            "Compact diff should have no context lines (lines starting with ' '), got: {context_lines_present:?}"
        );

        assert!(
            compact.contains("LINE3_CHANGED"),
            "Compact diff should contain the changed line: {compact}"
        );
    }

    #[test]
    fn staged_stat_returns_stat() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("stat_test.txt"), "some content\n").unwrap();
        Command::new("git")
            .args(["add", "stat_test.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let stat = staged_stat(path).unwrap();
        assert!(
            stat.contains("stat_test.txt"),
            "Stat should mention filename, got: {stat}"
        );
    }

    #[test]
    fn staged_summary_counts_correctly() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("summary.txt"), "line one\nline two\n").unwrap();
        Command::new("git")
            .args(["add", "summary.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let (files, insertions, _deletions) = staged_summary(path).unwrap();
        assert_eq!(files, 1, "Expected 1 staged file");
        assert!(insertions > 0, "Expected insertions > 0");
    }

    #[test]
    fn commit_creates_commit() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("commit_test.txt"), "content\n").unwrap();
        Command::new("git")
            .args(["add", "commit_test.txt"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();

        commit(path, "my test commit message").unwrap();

        // Verify by checking git log
        let log = git_in(path, &["log", "--oneline", "-1"]).unwrap();
        assert!(
            log.contains("my test commit message"),
            "Expected commit message in log, got: {log}"
        );
    }
}
