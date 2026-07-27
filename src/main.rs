use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::IsTerminal;

pub mod config;
pub mod diff;
pub mod git;
pub mod hook;
pub mod llm;
pub mod prompt;
pub mod scope;
pub mod setup;
pub mod token;
pub mod ui;

#[derive(Parser)]
#[command(name = "acm", about = "AI commit message generator", version)]
struct Cli {
    /// Skip confirmation, commit directly
    #[arg(short = 'y', long)]
    yes: bool,

    /// Provide context hint for the LLM
    #[arg(short, long)]
    context: Option<String>,

    /// Show message but don't commit
    #[arg(long)]
    dry_run: bool,

    /// Internal: used by prepare-commit-msg hook
    #[arg(long, hide = true)]
    hook: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_exposes_package_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn only_interactive_commit_flow_shows_diff_before_generation() {
        assert!(should_show_diff_before_generation(false, false, false));
        assert!(!should_show_diff_before_generation(true, false, false));
        assert!(!should_show_diff_before_generation(false, true, false));
        assert!(!should_show_diff_before_generation(false, false, true));
    }

    #[test]
    fn hook_flow_does_not_offer_to_stage_unstaged_changes() {
        assert!(!should_offer_stage_when_no_staged(true));
        assert!(should_offer_stage_when_no_staged(false));
    }

    #[test]
    fn unchanged_streamed_message_is_not_printed_again_on_a_terminal() {
        assert!(!should_print_final_to_stdout(
            true,
            "feat(.): ignore target and .codex files\n",
            "feat(.): ignore target and .codex files"
        ));
    }

    #[test]
    fn cleaned_message_is_reprinted_on_a_terminal_when_it_differs() {
        assert!(should_print_final_to_stdout(
            true,
            "Here is the message:\nfeat: add x",
            "feat: add x"
        ));
    }

    #[test]
    fn piped_stdout_always_receives_the_final_message() {
        // A consumer reading stdout must get exactly one complete message,
        // even when it matches what was streamed to stderr.
        assert!(should_print_final_to_stdout(
            false,
            "feat: add x",
            "feat: add x"
        ));
    }

    #[test]
    fn generated_message_must_be_non_empty_conventional_commit() {
        assert!(validate_generated_message("").is_err());
        assert!(validate_generated_message("update the readme").is_err());
        assert!(validate_generated_message("fix: handle empty provider output").is_ok());
    }
}

#[derive(Subcommand)]
enum Command {
    /// Configure acm
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Guided setup: pick provider, model, test connection
    Setup,
    /// Manage git hook
    Hook {
        #[command(subcommand)]
        action: HookAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Set a config value (e.g. model=llama3)
    Set {
        /// Key=value pair
        pair: String,
    },
    /// Show current config
    Show,
}

#[derive(Subcommand)]
enum HookAction {
    /// Install prepare-commit-msg git hook
    Install,
    /// Remove prepare-commit-msg hook
    Uninstall,
}

fn maybe_push(repo: &std::path::Path) -> Result<()> {
    if ui::prompt_push()? {
        git::push(repo)?;
    }
    Ok(())
}

fn should_show_diff_before_generation(yes: bool, dry_run: bool, hook: bool) -> bool {
    !yes && !dry_run && !hook
}

fn should_offer_stage_when_no_staged(hook: bool) -> bool {
    !hook
}

/// stdout carries the machine-readable result; the live stream goes to stderr.
///
/// When stdout is not a terminal (piped, redirected, captured by a script) the
/// cleaned message is always written, so consumers get exactly one complete,
/// validated message. On a terminal the user already watched it stream, so it
/// is reprinted only when cleaning actually changed it.
fn should_print_final_to_stdout(stdout_is_terminal: bool, displayed: &str, cleaned: &str) -> bool {
    !stdout_is_terminal || cleaned != displayed.trim()
}

fn validate_generated_message(message: &str) -> Result<()> {
    let message = message.trim();
    if message.is_empty() {
        anyhow::bail!("provider returned an empty commit message");
    }
    if !prompt::is_conventional_commit_message(message) {
        let first_line = message.lines().next().unwrap_or_default();
        anyhow::bail!("provider returned a non-conventional commit message: {first_line}");
    }
    Ok(())
}

/// Advisory: an over-long subject is still a usable commit, so this warns
/// rather than failing — aborting a commit (or a prepare-commit-msg hook) over
/// a cosmetic rule would be worse than the defect.
fn warn_if_subject_too_long(message: &str) {
    if prompt::first_line_too_long(message) {
        let n = message.lines().next().unwrap_or_default().chars().count();
        eprintln!(
            "  warning: subject is {n} characters (conventional limit is {}).",
            prompt::MAX_SUBJECT_CHARS
        );
    }
}

async fn generate(cli: &Cli) -> Result<()> {
    let cfg = config::load()?;
    let repo = git::repo_root()?;

    // If nothing staged, offer to stage all changes
    let files = match git::staged_files(&repo) {
        Ok(f) => f,
        Err(_) if cli.hook.is_some() => return Ok(()),
        Err(_)
            if should_offer_stage_when_no_staged(cli.hook.is_some())
                && git::has_unstaged_changes(&repo) =>
        {
            eprintln!("  No staged changes, but unstaged changes detected.");
            if let Ok(stat) = git::unstaged_stat(&repo) {
                ui::print_unstaged_stat(&stat);
            }
            if cli.yes || ui::prompt_stage()? {
                git::stage_all(&repo)?;
                git::staged_files(&repo)?
            } else {
                anyhow::bail!("no staged changes. Stage files with \"git add\" first.");
            }
        }
        Err(e) => return Err(e),
    };
    let (file_count, ins, del) = git::staged_summary(&repo)?;

    if !cli.dry_run && cli.hook.is_none() {
        ui::print_summary(file_count, ins, del);
    }

    let detected_scope = scope::detect_scope(&files);
    let system_prompt = prompt::build_system_prompt(&cfg, detected_scope.as_deref());
    let trailing_instruction = prompt::build_trailing_instruction(&cfg, detected_scope.as_deref());
    let (initial_diff, initial_mode) = diff::fit_diff(&repo, cfg.max_input_tokens, &cfg.diff_mode)?;

    let provider = llm::Provider::from_config(&cfg)?;

    if should_show_diff_before_generation(cli.yes, cli.dry_run, cli.hook.is_some()) {
        git::show_staged_diff_paged(&repo)?;
    }

    let mut current_diff = initial_diff;
    let mut current_mode = initial_mode;
    let show_thinking = !cli.yes && !cli.dry_run && cli.hook.is_none();

    loop {
        let mut user_content = current_diff.clone();
        if let Some(ctx) = &cli.context {
            user_content = format!("Context: {ctx}\n\n{user_content}");
        }
        // The format spec goes LAST, after the diff: local models drop
        // system-prompt rules across a long diff, and recency is what makes
        // them comply. Joined with a blank line because a truncated or stat
        // diff is not guaranteed to end in a newline.
        user_content = format!("{}\n\n{trailing_instruction}", user_content.trim_end());

        let messages = vec![
            llm::Message {
                role: llm::Role::System,
                content: system_prompt.clone(),
            },
            llm::Message {
                role: llm::Role::User,
                content: user_content,
            },
        ];

        let stream_result = provider.chat_stream(messages).await;
        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                let is_context_error = msg.contains("context")
                    || msg.contains("too long")
                    || msg.contains("maximum")
                    || msg.contains("token");
                // The mode ladder is finite (full → compact → budgeted →
                // stat → None) and get_forced_diff returns the mode it
                // ACTUALLY produced, so this always terminates.
                if is_context_error
                    && let Some(smaller_mode) = diff::next_smaller_mode(current_mode)
                {
                    let (smaller_diff, actual_mode) =
                        diff::get_forced_diff(&repo, smaller_mode, cfg.max_input_tokens)?;
                    current_diff = smaller_diff;
                    current_mode = actual_mode;
                    continue;
                }
                return Err(e);
            }
        };

        let streamed = ui::stream_message(&mut stream, show_thinking).await?;
        let message = prompt::clean_message(&streamed.raw);
        validate_generated_message(&message)?;
        warn_if_subject_too_long(&message);

        if should_print_final_to_stdout(
            std::io::stdout().is_terminal(),
            &streamed.displayed,
            &message,
        ) {
            println!("{message}");
        }

        // Hook mode: write to commit message file and exit
        if let Some(ref hook_path) = cli.hook {
            std::fs::write(hook_path, &message)?;
            return Ok(());
        }

        // Dry-run mode
        if cli.dry_run {
            return Ok(());
        }

        // Auto-confirm mode
        if cli.yes {
            git::commit(&repo, &message)?;
            maybe_push(&repo)?;
            return Ok(());
        }

        // Interactive mode
        match ui::prompt_action()? {
            ui::Action::Commit => {
                git::commit(&repo, &message)?;
                maybe_push(&repo)?;
                return Ok(());
            }
            ui::Action::Edit => {
                let edited = ui::edit_message(&message)?;
                if !edited.is_empty() {
                    git::commit(&repo, &edited)?;
                    maybe_push(&repo)?;
                }
                return Ok(());
            }
            ui::Action::Regenerate => {
                eprintln!();
                continue;
            }
            ui::Action::Cancel => {
                return Ok(());
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Config { action }) => match action {
            ConfigAction::Set { pair } => {
                let (key, value) = pair.split_once('=').context("expected key=value format")?;
                config::set_value(&config::config_path(), key.trim(), value.trim())?;
                eprintln!("set {} = {}", key.trim(), value.trim());
            }
            ConfigAction::Show => {
                let cfg = config::load()?;
                println!("{}", config::display(&cfg));
            }
        },
        Some(Command::Setup) => {
            setup::run().await?;
        }
        Some(Command::Hook { action }) => match action {
            HookAction::Install => hook::install()?,
            HookAction::Uninstall => hook::uninstall()?,
        },
        None => {
            generate(&cli).await?;
        }
    }

    Ok(())
}
