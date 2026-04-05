use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod config;
pub mod diff;
pub mod git;
pub mod scope;
pub mod token;

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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Command::Config { action }) => match action {
            ConfigAction::Set { pair } => {
                println!("config set: {pair}");
            }
            ConfigAction::Show => {
                println!("config show");
            }
        },
        Some(Command::Setup) => {
            println!("setup");
        }
        Some(Command::Hook { action }) => match action {
            HookAction::Install => println!("hook install"),
            HookAction::Uninstall => println!("hook uninstall"),
        },
        None => {
            println!("generate commit message (not yet implemented)");
        }
    }

    Ok(())
}
