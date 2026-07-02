use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

mod auth;
mod config;
mod letterboxd_export;
mod trakt_client;
mod trakt_read;
mod trakt_write;

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
    /// Show Trakt account status: username and movie counts
    TraktStatus,
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

const TRAKT_BASE_URL: &str = "https://api.trakt.tv";

fn run_trakt_status(cfg: &config::Config) -> Result<(), String> {
    let client = trakt_client::ReqwestClient::new(&cfg.trakt_client_id);
    let token = auth::get_valid_token(
        &client,
        &cfg.trakt_client_id,
        &cfg.trakt_client_secret,
        &cfg.data_dir,
    )?;

    let username = trakt_read::fetch_username(&client, TRAKT_BASE_URL, &token)?;
    let watched = trakt_read::fetch_watched_history(&client, TRAKT_BASE_URL, &token)?;
    let ratings = trakt_read::fetch_ratings(&client, TRAKT_BASE_URL, &token)?;
    let watchlist = trakt_read::fetch_watchlist(&client, TRAKT_BASE_URL, &token)?;

    println!("Authenticated as: {username}");
    println!("Watched movies:   {}", watched.len());
    println!("Rated movies:     {}", ratings.len());
    println!("Watchlist movies: {}", watchlist.len());

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let cfg = match config::Config::load(cli.config.as_deref()) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    match &cli.command {
        Command::Auth => {
            let client = trakt_client::ReqwestClient::new(&cfg.trakt_client_id);
            match auth::run_device_flow(
                &client,
                &cfg.trakt_client_id,
                &cfg.trakt_client_secret,
                &cfg.data_dir,
            ) {
                Ok(_) => println!("Authorization successful."),
                Err(e) => {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            }
        }
        Command::TraktStatus => {
            if let Err(e) = run_trakt_status(&cfg) {
                eprintln!("error: {e}");
                process::exit(1);
            }
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
