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
    Budgeted,
    Stat,
}

impl fmt::Display for DiffMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiffMode::Full => write!(f, "full"),
            DiffMode::Compact => write!(f, "compact"),
            DiffMode::Budgeted => write!(f, "budgeted"),
            DiffMode::Stat => write!(f, "stat"),
        }
    }
}

impl std::str::FromStr for DiffMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "full" => Ok(DiffMode::Full),
            "compact" => Ok(DiffMode::Compact),
            "budgeted" => Ok(DiffMode::Budgeted),
            "stat" => Ok(DiffMode::Stat),
            _ => Err(()),
        }
    }
}

/// Get the next smaller diff mode for retry on context-length error.
pub fn next_smaller_mode(mode: DiffMode) -> Option<DiffMode> {
    match mode {
        DiffMode::Full => Some(DiffMode::Compact),
        DiffMode::Compact => Some(DiffMode::Budgeted),
        DiffMode::Budgeted => Some(DiffMode::Stat),
        DiffMode::Stat => None,
    }
}

// ---------------------------------------------------------------------------
// Budgeted diff builder (pure)
// ---------------------------------------------------------------------------

/// Minimum per-file budget worth spending: below this, a file's hunks carry
/// too little content to inform the message.
const FLOOR_TOKENS: usize = 120;
/// Reserved up front for the trailing omission summary line.
const OMISSION_RESERVE: usize = 24;
const TRUNCATION_MARKER: &str = "... [hunks truncated]";

fn budgeted_header(stat: &str) -> String {
    format!(
        "Files changed (ranked by relevance):\n{stat}\n\n--- diff (hunks by relevance; some truncated or omitted) ---\n"
    )
}

/// Append one file's diff to `sections`, whole if it fits `cap`, otherwise
/// truncated with a marker. Returns false (and appends nothing) when even a
/// truncated section is not worth emitting.
fn append_section(
    diff: &str,
    cap: usize,
    sections: &mut Vec<String>,
    remaining: &mut usize,
) -> bool {
    if diff.trim().is_empty() {
        return false;
    }
    // +1 accounts for the joining newline between sections.
    let whole_cost = token::estimate_tokens(diff) + 1;
    if whole_cost <= cap {
        sections.push(diff.to_string());
        *remaining = remaining.saturating_sub(whole_cost);
        return true;
    }
    let marker_cost = token::estimate_tokens(TRUNCATION_MARKER) + 1;
    if cap <= marker_cost {
        return false;
    }
    let truncated = token::truncate_to_tokens(diff, cap - marker_cost);
    if truncated.trim().is_empty() {
        return false;
    }
    let section = format!("{}\n{}", truncated.trim_end(), TRUNCATION_MARKER);
    *remaining = remaining.saturating_sub(token::estimate_tokens(&section) + 1);
    sections.push(section);
    true
}

/// Build a budgeted diff from relevance-ranked per-file compact diffs.
///
/// Two-pass greedy allocation on the exact emitted text:
/// * Pass 1 — high-signal files, in order, each getting
///   `min(remaining, max(FLOOR, remaining / files_left))` tokens.
/// * Pass 2 — low-signal files (lockfiles, generated assets) receive hunks
///   only from whatever budget is left after every signal file was visited.
///
/// Files whose hunks were dropped entirely are counted in a trailing
/// omission line. Returns `None` when the stat header leaves no usable
/// budget for any hunks — callers should fall back to stat.
pub fn build_budgeted(
    stat: &str,
    per_file: &[(String, String)],
    max_tokens: usize,
) -> Option<String> {
    if per_file.is_empty() {
        return None;
    }
    let header = budgeted_header(stat);
    let fixed = token::estimate_tokens(&header) + OMISSION_RESERVE;
    let mut remaining = max_tokens.checked_sub(fixed)?;
    if remaining < FLOOR_TOKENS {
        return None;
    }

    let (signal, low): (Vec<_>, Vec<_>) = per_file
        .iter()
        .partition(|(path, _)| !git::is_low_signal_path(path));

    let mut sections: Vec<String> = Vec::new();
    let mut omitted = 0usize;

    for (i, (_, diff)) in signal.iter().enumerate() {
        if remaining < FLOOR_TOKENS {
            omitted += 1;
            continue;
        }
        let files_left = signal.len() - i;
        let cap = remaining.min(FLOOR_TOKENS.max(remaining / files_left));
        if !append_section(diff, cap, &mut sections, &mut remaining) {
            omitted += 1;
        }
    }
    for (_, diff) in &low {
        if remaining < FLOOR_TOKENS {
            omitted += 1;
            continue;
        }
        if !append_section(diff, remaining, &mut sections, &mut remaining) {
            omitted += 1;
        }
    }

    if sections.is_empty() {
        return None;
    }
    let mut out = header;
    out.push_str(&sections.join("\n"));
    if omitted > 0 {
        out.push_str(&format!(
            "\n[hunks omitted for {omitted} of {} files — see file list above]",
            per_file.len()
        ));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Git-backed helpers
// ---------------------------------------------------------------------------

/// Wrap a diff body with a ranked stat header so the model always sees the
/// list of changed files (even if the body gets truncated downstream).
pub fn format_with_stat_header(stat: &str, diff_body: &str) -> String {
    if stat.is_empty() {
        return diff_body.to_string();
    }
    if diff_body.is_empty() {
        return stat.to_string();
    }
    format!(
        "Files changed (ranked by relevance):\n{stat}\n\n--- diff (most relevant first) ---\n{diff_body}"
    )
}

/// Per-file compact (zero-context) diffs in relevance order.
fn per_file_compact_diffs(repo: &Path) -> Result<Vec<(String, String)>> {
    let ranked = git::staged_files_ranked(repo)?;
    let mut out = Vec::with_capacity(ranked.len());
    for f in ranked {
        let diff = git::staged_diff_for_files(repo, Some(0), std::slice::from_ref(&f))?;
        out.push((f, diff));
    }
    Ok(out)
}

/// Fetch the diff for a specific mode from the git index. Returns the text
/// and the mode ACTUALLY produced: a forced/retried `Budgeted` that is not
/// viable within `max_tokens` degrades to (truncated) `Stat`, and the
/// returned mode says so — callers must trust it for retry progression.
pub fn get_forced_diff(repo: &Path, mode: DiffMode, max_tokens: usize) -> Result<(String, DiffMode)> {
    let stat = git::staged_stat_ranked(repo)?;
    match mode {
        DiffMode::Stat => Ok((token::truncate_to_tokens(&stat, max_tokens), DiffMode::Stat)),
        DiffMode::Budgeted => {
            let per_file = per_file_compact_diffs(repo)?;
            match build_budgeted(&stat, &per_file, max_tokens) {
                Some(text) => Ok((text, DiffMode::Budgeted)),
                None => Ok((token::truncate_to_tokens(&stat, max_tokens), DiffMode::Stat)),
            }
        }
        DiffMode::Full | DiffMode::Compact => {
            let ranked = git::staged_files_ranked(repo)?;
            let ctx = if mode == DiffMode::Compact { Some(0) } else { None };
            let body = git::staged_diff_for_files(repo, ctx, &ranked)?;
            Ok((format_with_stat_header(&stat, &body), mode))
        }
    }
}

/// Top-level function.
///
/// * If `forced_mode` is not `"auto"`, fetch that specific diff.
/// * Otherwise walk the ladder full → compact → budgeted → stat, returning
///   the first variant that fits `max_tokens`.
///
/// Files are ordered by relevance (lines changed, low-signal files demoted),
/// and a ranked stat header is prepended so the model can reason about every
/// changed file even when hunks are truncated.
pub fn fit_diff(repo: &Path, max_tokens: usize, forced_mode: &str) -> Result<(String, DiffMode)> {
    if forced_mode != "auto" {
        let mode = forced_mode
            .parse::<DiffMode>()
            .map_err(|_| anyhow::anyhow!("unknown diff mode: {}", forced_mode))?;
        return get_forced_diff(repo, mode, max_tokens);
    }

    let stat = git::staged_stat_ranked(repo)?;
    let per_file = per_file_compact_diffs(repo)?;
    let ranked: Vec<String> = per_file.iter().map(|(f, _)| f.clone()).collect();

    let full = format_with_stat_header(&stat, &git::staged_diff_for_files(repo, None, &ranked)?);
    if token::estimate_tokens(&full) <= max_tokens {
        return Ok((full, DiffMode::Full));
    }

    let compact_body: Vec<&str> = per_file
        .iter()
        .map(|(_, d)| d.as_str())
        .filter(|d| !d.is_empty())
        .collect();
    let compact = format_with_stat_header(&stat, &compact_body.join("\n"));
    if token::estimate_tokens(&compact) <= max_tokens {
        return Ok((compact, DiffMode::Compact));
    }

    if let Some(budgeted) = build_budgeted(&stat, &per_file, max_tokens) {
        return Ok((budgeted, DiffMode::Budgeted));
    }

    Ok((token::truncate_to_tokens(&stat, max_tokens), DiffMode::Stat))
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
    fn mode_ladder_terminates() {
        // full → compact → budgeted → stat → None; guards against a future
        // enum change creating an infinite provider-retry loop.
        assert_eq!(next_smaller_mode(DiffMode::Full), Some(DiffMode::Compact));
        assert_eq!(
            next_smaller_mode(DiffMode::Compact),
            Some(DiffMode::Budgeted)
        );
        assert_eq!(next_smaller_mode(DiffMode::Budgeted), Some(DiffMode::Stat));
        assert_eq!(next_smaller_mode(DiffMode::Stat), None);
    }

    #[test]
    fn budgeted_mode_parses() {
        assert_eq!("budgeted".parse::<DiffMode>(), Ok(DiffMode::Budgeted));
        assert_eq!(DiffMode::Budgeted.to_string(), "budgeted");
    }

    // -----------------------------------------------------------------------
    // Unit tests for build_budgeted (pure, no git)
    // -----------------------------------------------------------------------

    fn fake_diff(path: &str, lines: usize) -> (String, String) {
        let mut d = format!("diff --git a/{path} b/{path}\n@@ -0,0 +1,{lines} @@\n");
        for i in 0..lines {
            d.push_str(&format!("+content line {i} in {path}\n"));
        }
        (path.to_string(), d)
    }

    #[test]
    fn budgeted_none_when_header_exceeds_budget() {
        let stat = " some-file.rs | 999 (+999 -0)\n".repeat(200);
        let per_file = vec![fake_diff("src/a.rs", 50)];
        assert_eq!(build_budgeted(&stat, &per_file, 100), None);
    }

    #[test]
    fn budgeted_none_when_no_files() {
        assert_eq!(build_budgeted("stat", &[], 4096), None);
    }

    #[test]
    fn budgeted_output_fits_budget_and_marks_truncation() {
        let stat = " src/a.rs | 2000  (+2000 -0)";
        let per_file = vec![fake_diff("src/a.rs", 2000)];
        let max_tokens = 600;
        let out = build_budgeted(stat, &per_file, max_tokens).expect("should be viable");
        assert!(
            token::estimate_tokens(&out) <= max_tokens,
            "budgeted output must fit: {} > {max_tokens}",
            token::estimate_tokens(&out)
        );
        assert!(
            out.contains(TRUNCATION_MARKER),
            "oversized file should carry truncation marker:\n{out}"
        );
        assert!(out.contains("content line 0 in src/a.rs"));
    }

    #[test]
    fn budgeted_signal_files_win_over_bigger_lockfile() {
        // Ranked input: signal files first (as git::staged_files_ranked
        // produces), lockfile last despite being biggest.
        let stat = " x";
        let per_file = vec![
            fake_diff("src/a.rs", 100),
            fake_diff("src/b.rs", 100),
            fake_diff("Cargo.lock", 3000),
        ];
        let out = build_budgeted(stat, &per_file, 800).expect("viable");
        assert!(out.contains("content line 0 in src/a.rs"));
        assert!(out.contains("content line 0 in src/b.rs"));
        // Lockfile gets at most leftovers; if dropped, the omission line says so.
        if !out.contains("content line 0 in Cargo.lock") {
            assert!(
                out.contains("[hunks omitted for 1 of 3 files"),
                "dropped lockfile must be reported:\n{out}"
            );
        }
        assert!(token::estimate_tokens(&out) <= 800);
    }

    #[test]
    fn budgeted_all_low_signal_diff_still_gets_hunks() {
        let stat = " Cargo.lock | 50  (+50 -0) (low-signal)";
        let per_file = vec![fake_diff("Cargo.lock", 50)];
        let out = build_budgeted(stat, &per_file, 2048).expect("viable");
        assert!(
            out.contains("content line 0 in Cargo.lock"),
            "all-low-signal commit should include its hunks:\n{out}"
        );
    }

    #[test]
    fn budgeted_reports_omitted_files() {
        let stat = " x";
        // More files than the budget can give a floor-sized slice to:
        // the tail must be counted in the omission line.
        let per_file: Vec<(String, String)> = (0..8)
            .map(|i| fake_diff(&format!("src/f{i}.rs"), 2000))
            .collect();
        let out = build_budgeted(stat, &per_file, 500).expect("viable");
        assert!(token::estimate_tokens(&out) <= 500);
        assert!(
            out.contains("[hunks omitted for") && out.contains("of 8 files"),
            "omitted files must be reported:\n{out}"
        );
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

    fn stage_lines(path: &std::path::Path, name: &str, lines: usize) {
        let mut content = String::new();
        for i in 0..lines {
            content.push_str(&format!("line {i} of {name}\n"));
        }
        fs::write(path.join(name), &content).unwrap();
        Command::new("git")
            .args(["add", name])
            .current_dir(path)
            .output()
            .unwrap();
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

    #[test]
    fn fit_diff_auto_picks_budgeted_when_compact_too_big() {
        let repo = init_repo();
        let path = repo.path();
        stage_lines(path, "big.txt", 2000);

        let (diff, mode) = fit_diff(path, 600, "auto").expect("fit_diff failed");
        assert_eq!(mode, DiffMode::Budgeted, "got:\n{diff}");
        assert!(token::estimate_tokens(&diff) <= 600);
        assert!(
            diff.contains("line 0 of big.txt"),
            "budgeted diff should contain top-ranked hunks:\n{diff}"
        );
    }

    #[test]
    fn forced_budgeted_returns_budgeted_when_viable() {
        let repo = init_repo();
        let path = repo.path();
        stage_lines(path, "a.txt", 30);

        let (diff, mode) = get_forced_diff(path, DiffMode::Budgeted, 4096).unwrap();
        assert_eq!(mode, DiffMode::Budgeted);
        assert!(diff.contains("line 0 of a.txt"));
    }

    #[test]
    fn forced_budgeted_falls_back_to_truncated_stat_when_not_viable() {
        let repo = init_repo();
        let path = repo.path();
        stage_lines(path, "a.txt", 30);

        let max_tokens = 20;
        let (diff, mode) = get_forced_diff(path, DiffMode::Budgeted, max_tokens).unwrap();
        assert_eq!(
            mode,
            DiffMode::Stat,
            "not-viable budgeted must be labeled Stat so retries terminate"
        );
        assert!(
            token::estimate_tokens(&diff) <= max_tokens,
            "fallback stat must be truncated to budget"
        );
    }

    #[test]
    fn fit_diff_includes_stat_header_and_ranks_largest_first() {
        let repo = init_repo();
        let path = repo.path();

        // Alphabetically, "a_small.txt" comes first — but "z_big.txt" has
        // many more changes, so it should win the top spot in both the
        // stat header and the diff body.
        fs::write(path.join("a_small.txt"), "x\n").unwrap();
        let mut big = String::new();
        for i in 0..40 {
            big.push_str(&format!("line {i}\n"));
        }
        fs::write(path.join("z_big.txt"), &big).unwrap();
        Command::new("git")
            .args(["add", "a_small.txt", "z_big.txt"])
            .current_dir(path)
            .output()
            .unwrap();

        let (diff, mode) = fit_diff(path, 8192, "auto").expect("fit_diff failed");
        assert_eq!(mode, DiffMode::Full, "small change set should fit as Full");

        assert!(
            diff.starts_with("Files changed (ranked by relevance):"),
            "diff should start with ranked stat header, got:\n{diff}"
        );
        assert!(
            diff.contains("--- diff"),
            "diff should contain the diff-section separator, got:\n{diff}"
        );

        let big_pos = diff.find("z_big.txt").expect("z_big.txt missing");
        let small_pos = diff.find("a_small.txt").expect("a_small.txt missing");
        assert!(
            big_pos < small_pos,
            "z_big.txt should appear before a_small.txt in the combined output"
        );
    }

    #[test]
    fn format_with_stat_header_combines_in_order() {
        let result = format_with_stat_header("STAT-LINE", "DIFF-BODY");
        let stat_pos = result.find("STAT-LINE").unwrap();
        let diff_pos = result.find("DIFF-BODY").unwrap();
        assert!(stat_pos < diff_pos);
    }

    #[test]
    fn format_with_stat_header_handles_empty_stat() {
        assert_eq!(format_with_stat_header("", "DIFF"), "DIFF");
    }
}
