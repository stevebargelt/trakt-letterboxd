use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

mod config;

#[derive(Parser)]
#[command(
    name = "trakt-letterboxd",
    about = "Sync Trakt \u{2194} Letterboxd watched history, ratings, and watchlists"
)]
struct Cli {
    /// Path to config file (default: ~/.config/trakt-letterboxd/config.toml)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authorize with Trakt via OAuth device flow
    Auth,
    /// Sync between Trakt and Letterboxd
    Sync {
        #[command(subcommand)]
        direction: SyncDirection,
    },
}

#[derive(Subcommand)]
enum SyncDirection {
    /// Import a Letterboxd export ZIP into Trakt
    FromLetterboxd {
        /// Path to Letterboxd export ZIP or extracted directory
        path: PathBuf,
    },
    /// Generate a Letterboxd import CSV from Trakt data
    ToLetterboxd,
}

fn main() {
    let cli = Cli::parse();

    let _cfg = match config::Config::load(cli.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    match &cli.command {
        Command::Auth => {
            println!("auth: not yet implemented");
        }
        Command::Sync { direction } => match direction {
            SyncDirection::FromLetterboxd { .. } => {
                println!("sync from-letterboxd: not yet implemented");
            }
            SyncDirection::ToLetterboxd => {
                println!("sync to-letterboxd: not yet implemented");
            }
        },
    }
}
