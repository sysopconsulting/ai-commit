use anyhow::{Context, Result, bail};
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

fn git_in_raw(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;

    if output.status.success() {
        Ok(output.stdout)
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

/// Returns the staged diff for the given files in the order provided.
/// One git invocation per file so the requested order is preserved
/// (git's normal `--` pathspec filtering does not preserve argument order).
pub fn staged_diff_for_files(
    repo: &Path,
    context_lines: Option<u32>,
    files: &[String],
) -> Result<String> {
    let mut combined = String::new();
    for f in files {
        let mut args: Vec<String> = vec!["diff".into(), "--cached".into()];
        if let Some(n) = context_lines {
            args.push(format!("--unified={n}"));
        }
        args.push("--".into());
        args.push(f.clone());
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let part = git_in(repo, &args_ref)?;
        if part.is_empty() {
            continue;
        }
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&part);
    }
    Ok(combined)
}

/// Returns per-file numstat for staged changes: `(insertions, deletions, filename)`.
/// Binary files report `0, 0` (their numstat is `-`).
pub fn staged_numstat(repo: &Path) -> Result<Vec<(usize, usize, String)>> {
    let output = git_in_raw(repo, &["diff", "--cached", "--numstat", "-z"])?;
    let output = String::from_utf8_lossy(&output);
    let mut entries = Vec::new();
    let mut fields = output.split_terminator('\0');

    while let Some(record) = fields.next() {
        let parts: Vec<&str> = record.splitn(3, '\t').collect();
        if parts.len() < 3 {
            continue;
        }

        let added: usize = parts[0].parse().unwrap_or(0);
        let removed: usize = parts[1].parse().unwrap_or(0);
        let path = if parts[2].is_empty() {
            let _old_path = fields.next();
            fields.next().unwrap_or_default()
        } else {
            parts[2]
        };

        if !path.is_empty() {
            entries.push((added, removed, path.to_string()));
        }
    }
    Ok(entries)
}

/// True for files whose diff content is near-certain noise for commit-message
/// purposes: lockfiles, minified assets, source maps, and vendored dependency
/// directories. Matching is conservative — exact lockfile basenames, specific
/// extensions, and `node_modules`/`vendor` as directory components only (a
/// root-level file literally named `vendor` is not demoted).
pub fn is_low_signal_path(path: &str) -> bool {
    const LOCKFILES: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "bun.lockb",
        "go.sum",
        "composer.lock",
        "Gemfile.lock",
        "poetry.lock",
        "uv.lock",
    ];
    let p = Path::new(path);
    let Some(basename) = p.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return false;
    };
    if LOCKFILES.contains(&basename.as_str()) {
        return true;
    }
    let lower = basename.to_lowercase();
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") || lower.ends_with(".map") {
        return true;
    }
    if let Some(parent) = p.parent() {
        for component in parent.components() {
            let c = component.as_os_str().to_string_lossy();
            if c == "node_modules" || c == "vendor" {
                return true;
            }
        }
    }
    false
}

/// Sort numstat entries by relevance: high-signal files first (by lines
/// changed, descending), then low-signal files (same order). Ties break
/// alphabetically for stable output.
fn rank_entries(entries: &mut [(usize, usize, String)]) {
    entries.sort_by(|a, b| {
        let (la, lb) = (is_low_signal_path(&a.2), is_low_signal_path(&b.2));
        la.cmp(&lb)
            .then_with(|| (b.0 + b.1).cmp(&(a.0 + a.1)))
            .then_with(|| a.2.cmp(&b.2))
    });
}

/// Returns staged file paths ordered by relevance: high-signal files by lines
/// changed (descending), then low-signal files (lockfiles, generated assets).
pub fn staged_files_ranked(repo: &Path) -> Result<Vec<String>> {
    let mut entries = staged_numstat(repo)?;
    rank_entries(&mut entries);
    Ok(entries.into_iter().map(|(_, _, f)| f).collect())
}

/// Custom stat header listing files ranked by relevance (see [`rank_entries`]),
/// followed by the standard "N files changed, X insertions(+), Y deletions(-)" summary.
/// Low-signal files are marked so the model can discount them.
pub fn staged_stat_ranked(repo: &Path) -> Result<String> {
    let mut entries = staged_numstat(repo)?;
    if entries.is_empty() {
        return Ok(String::new());
    }
    rank_entries(&mut entries);
    let name_w = entries.iter().map(|(_, _, n)| n.len()).max().unwrap_or(0);
    let total_w = entries
        .iter()
        .map(|(a, d, _)| (a + d).to_string().len())
        .max()
        .unwrap_or(1);
    let mut out = String::new();
    let (mut tot_ins, mut tot_del) = (0usize, 0usize);
    for (added, removed, name) in &entries {
        let total = added + removed;
        let marker = if is_low_signal_path(name) {
            " (low-signal)"
        } else {
            ""
        };
        out.push_str(&format!(
            " {name:<name_w$} | {total:>total_w$}  (+{added} -{removed}){marker}\n",
        ));
        tot_ins += added;
        tot_del += removed;
    }
    out.push_str(&format!(
        " {} file{} changed, {} insertion{}(+), {} deletion{}(-)",
        entries.len(),
        if entries.len() == 1 { "" } else { "s" },
        tot_ins,
        if tot_ins == 1 { "" } else { "s" },
        tot_del,
        if tot_del == 1 { "" } else { "s" },
    ));
    Ok(out)
}

/// Returns `(file_count, total_insertions, total_deletions)` from `git diff --cached --numstat`.
pub fn staged_summary(repo: &Path) -> Result<(usize, usize, usize)> {
    let entries = staged_numstat(repo)?;
    let files = entries.len();
    let insertions: usize = entries.iter().map(|(a, _, _)| *a).sum();
    let deletions: usize = entries.iter().map(|(_, d, _)| *d).sum();
    Ok((files, insertions, deletions))
}

/// Returns `git status --porcelain` output (one line per changed/untracked file).
pub fn working_tree_status(repo: &Path) -> Result<String> {
    git_in(repo, &["status", "--porcelain"])
}

/// Returns `git diff --cached --stat` output.
pub fn unstaged_stat(repo: &Path) -> Result<String> {
    let mut output = git_in(repo, &["diff", "--stat"])?;
    let untracked = git_in(repo, &["ls-files", "--others", "--exclude-standard"])?;
    if !untracked.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        for file in untracked.lines() {
            output.push_str(&format!(" {file} | untracked\n"));
        }
        output = output.trim_end().to_string();
    }
    Ok(output)
}

/// Show the staged diff in a pager (`$PAGER` or `less -R`).
/// Blocks until the user quits the pager.
pub fn show_staged_diff_paged(repo: &Path) -> Result<()> {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less -R".into());
    let parts: Vec<&str> = pager.split_whitespace().collect();
    let (cmd, args) = parts.split_first().unwrap_or((&"less", &["-R"][..]));

    let diff_output = std::process::Command::new("git")
        .args(["diff", "--cached", "--color=always"])
        .current_dir(repo)
        .output()
        .context("failed to run git diff --cached")?;

    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn pager: {pager}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(&diff_output.stdout);
    }

    child.wait().context("pager exited with error")?;
    Ok(())
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
    let output = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(repo)
        .env("ACM_SKIP_HOOK", "1")
        .output()
        .context("Failed to run: git commit")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git commit: {}", stderr)
    }
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

/// Resolve a path inside Git's private directory, including linked worktrees.
pub fn git_path(repo: &Path, pathspec: &str) -> Result<PathBuf> {
    let output = git_in(repo, &["rev-parse", "--git-path", pathspec])?;
    let path = PathBuf::from(output);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(repo.join(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::{TempDir, tempdir};

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

        let context_lines_present: Vec<&str> =
            compact.lines().filter(|l| l.starts_with(' ')).collect();
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
    fn staged_stat_ranked_lists_files_in_descending_change_order() {
        let repo = init_repo();
        let path = repo.path();

        // Big file: 20 lines added. Small file: 1 line added.
        let mut big = String::new();
        for i in 0..20 {
            big.push_str(&format!("line {i}\n"));
        }
        fs::write(path.join("small.txt"), "x\n").unwrap();
        fs::write(path.join("big.txt"), &big).unwrap();
        Command::new("git")
            .args(["add", "small.txt", "big.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let stat = staged_stat_ranked(path).unwrap();
        let big_pos = stat.find("big.txt").expect("big.txt missing from stat");
        let small_pos = stat.find("small.txt").expect("small.txt missing from stat");
        assert!(
            big_pos < small_pos,
            "big.txt should appear before small.txt in ranked stat:\n{stat}"
        );
        assert!(
            stat.contains("2 files changed"),
            "stat should include summary line, got:\n{stat}"
        );
    }

    #[test]
    fn staged_files_ranked_orders_by_change_size() {
        let repo = init_repo();
        let path = repo.path();

        let mut big = String::new();
        for i in 0..50 {
            big.push_str(&format!("line {i}\n"));
        }
        fs::write(path.join("a_small.txt"), "x\n").unwrap();
        fs::write(path.join("z_big.txt"), &big).unwrap();
        Command::new("git")
            .args(["add", "a_small.txt", "z_big.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let ranked = staged_files_ranked(path).unwrap();
        assert_eq!(
            ranked,
            vec!["z_big.txt".to_string(), "a_small.txt".to_string()]
        );
    }

    #[test]
    fn staged_diff_for_files_preserves_order() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("alpha.txt"), "alpha-content\n").unwrap();
        fs::write(path.join("zulu.txt"), "zulu-content\n").unwrap();
        Command::new("git")
            .args(["add", "alpha.txt", "zulu.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let diff = staged_diff_for_files(
            path,
            None,
            &["zulu.txt".to_string(), "alpha.txt".to_string()],
        )
        .unwrap();

        let zulu_pos = diff.find("zulu.txt").expect("zulu.txt missing");
        let alpha_pos = diff.find("alpha.txt").expect("alpha.txt missing");
        assert!(
            zulu_pos < alpha_pos,
            "diff should respect requested file order, got:\n{diff}"
        );
    }

    #[test]
    fn staged_files_ranked_uses_postimage_path_for_renames() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("old.txt"), "line one\nline two\n").unwrap();
        Command::new("git")
            .args(["add", "old.txt"])
            .current_dir(path)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "add old file"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test User")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test User")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();

        Command::new("git")
            .args(["mv", "old.txt", "new.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let ranked = staged_files_ranked(path).unwrap();
        assert_eq!(ranked, vec!["new.txt".to_string()]);

        let diff = staged_diff_for_files(path, None, &ranked).unwrap();
        assert!(
            diff.contains("rename to new.txt") || diff.contains("diff --git"),
            "ranked rename path should produce a staged diff, got:\n{diff}"
        );
    }

    #[test]
    fn unstaged_stat_includes_untracked_files() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("untracked.txt"), "new content\n").unwrap();

        let stat = unstaged_stat(path).unwrap();
        assert!(
            stat.contains("untracked.txt"),
            "Unstaged stat should mention untracked file, got: {stat:?}"
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

    #[test]
    fn low_signal_matches_lockfiles_by_basename() {
        assert!(is_low_signal_path("Cargo.lock"));
        assert!(is_low_signal_path("packages/foo/package-lock.json"));
        assert!(is_low_signal_path("a/b/yarn.lock"));
        assert!(is_low_signal_path("assets/app.min.js"));
        assert!(is_low_signal_path("dist/app.js.map"));
        assert!(is_low_signal_path("vendor/lib/util.go"));
        assert!(is_low_signal_path("pkg/node_modules/x/index.js"));
    }

    #[test]
    fn low_signal_does_not_match_regular_files() {
        assert!(!is_low_signal_path("src/main.rs"));
        assert!(!is_low_signal_path("Cargo.toml"));
        assert!(!is_low_signal_path("vendor")); // root file named vendor
        assert!(!is_low_signal_path("src/locks.rs"));
        assert!(!is_low_signal_path("docs/minjs.md"));
    }

    #[test]
    fn ranking_demotes_low_signal_files_below_smaller_signal_files() {
        let mut entries = vec![
            (500, 300, "Cargo.lock".to_string()),
            (10, 2, "src/main.rs".to_string()),
            (40, 5, "src/diff.rs".to_string()),
        ];
        rank_entries(&mut entries);
        let names: Vec<&str> = entries.iter().map(|(_, _, n)| n.as_str()).collect();
        assert_eq!(names, vec!["src/diff.rs", "src/main.rs", "Cargo.lock"]);
    }

    #[test]
    fn stat_ranked_marks_low_signal_files() {
        let repo = init_repo();
        let path = repo.path();

        fs::write(path.join("Cargo.lock"), "big lockfile content\n".repeat(30)).unwrap();
        fs::write(path.join("main.rs"), "fn main() {}\n").unwrap();
        Command::new("git")
            .args(["add", "Cargo.lock", "main.rs"])
            .current_dir(path)
            .output()
            .unwrap();

        let stat = staged_stat_ranked(path).unwrap();
        let lock_line = stat
            .lines()
            .find(|l| l.contains("Cargo.lock"))
            .expect("Cargo.lock missing from stat");
        assert!(
            lock_line.contains("(low-signal)"),
            "lockfile line should carry low-signal marker:\n{stat}"
        );
        let main_pos = stat.find("main.rs").unwrap();
        let lock_pos = stat.find("Cargo.lock").unwrap();
        assert!(
            main_pos < lock_pos,
            "signal file should outrank bigger lockfile:\n{stat}"
        );
    }

    #[test]
    fn commit_skips_acm_prepare_commit_msg_hook() {
        let repo = init_repo();
        let path = repo.path();
        let hooks_dir = path.join(".git/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(
            hooks_dir.join("prepare-commit-msg"),
            "#!/bin/sh\n[ \"$ACM_SKIP_HOOK\" = \"1\" ] || exit 42\n",
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let hook = hooks_dir.join("prepare-commit-msg");
            let mut perms = fs::metadata(&hook).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&hook, perms).unwrap();
        }

        fs::write(path.join("skip_hook.txt"), "content\n").unwrap();
        Command::new("git")
            .args(["add", "skip_hook.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        commit(path, "commit through acm").unwrap();
    }
}
