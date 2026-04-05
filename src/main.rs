use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
#[command(name = "acm", about = "AI commit message generator")]
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

async fn generate(cli: &Cli) -> Result<()> {
    let cfg = config::load()?;
    let repo = git::repo_root()?;
    let files = git::staged_files(&repo)?;
    let (file_count, ins, del) = git::staged_summary(&repo)?;

    if !cli.dry_run && cli.hook.is_none() {
        ui::print_summary(file_count, ins, del);
    }

    let detected_scope = scope::detect_scope(&files);
    let system_prompt = prompt::build_system_prompt(&cfg, detected_scope.as_deref());
    let (initial_diff, initial_mode) = diff::fit_diff(&repo, cfg.max_input_tokens, &cfg.diff_mode)?;

    let provider = llm::Provider::from_config(&cfg)?;

    let mut current_diff = initial_diff;
    let mut current_mode = initial_mode;
    let mut retries: u32 = 0;

    loop {
        let mut user_content = current_diff.clone();
        if let Some(ctx) = &cli.context {
            user_content = format!("Context: {ctx}\n\n{user_content}");
        }

        let messages = vec![
            llm::Message { role: llm::Role::System, content: system_prompt.clone() },
            llm::Message { role: llm::Role::User, content: user_content },
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
                if is_context_error && retries < 2 {
                    if let Some(smaller_mode) = diff::next_smaller_mode(current_mode) {
                        current_diff = diff::get_forced_diff(&repo, smaller_mode)?;
                        current_mode = smaller_mode;
                        retries += 1;
                        continue;
                    }
                }
                return Err(e);
            }
        };

        let message = ui::stream_message(&mut stream).await?;

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
            return Ok(());
        }

        // Interactive mode
        match ui::prompt_action()? {
            ui::Action::Commit => {
                git::commit(&repo, &message)?;
                return Ok(());
            }
            ui::Action::Edit => {
                let edited = ui::edit_message(&message)?;
                if !edited.is_empty() {
                    git::commit(&repo, &edited)?;
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
                let (key, value) = pair.split_once('=')
                    .context("expected key=value format")?;
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
