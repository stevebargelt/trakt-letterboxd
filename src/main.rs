use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process;

mod auth;
mod config;
mod letterboxd_export;
mod letterboxd_import;
mod matching;
mod rating;
mod sync_from_letterboxd;
mod sync_state;
mod sync_to_letterboxd;
mod trakt_client;
mod trakt_notes;
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
        /// Parse and report what would be synced without writing to Trakt
        #[arg(long)]
        dry_run: bool,
        /// Re-sync items even if already recorded in local state
        #[arg(long)]
        force: bool,
    },
    /// Generate a Letterboxd import CSV from Trakt data
    ToLetterboxd {
        /// Read Trakt data and compute counts, but write no files and do not update sync state
        #[arg(long)]
        dry_run: bool,
        /// Re-export everything, ignoring previously exported items
        #[arg(long)]
        force: bool,
    },
}

const TRAKT_BASE_URL: &str = "https://api.trakt.tv";

fn print_from_letterboxd_summary(s: &sync_from_letterboxd::SyncSummary) {
    if s.dry_run {
        println!("[DRY RUN] Letterboxd \u{2192} Trakt (no changes written)");
    } else {
        println!("Letterboxd \u{2192} Trakt sync complete");
    }
    println!();
    println!("  Watched history: {}", s.watched_added);
    println!("  Ratings:         {}", s.ratings_added);
    println!("  Watchlist:       {}", s.watchlist_added);
    println!("  Already synced:  {} skipped", s.skipped);
    println!("  Reviews:         {} transferred", s.reviews_transferred);
    if s.reviews_skipped_unmatched > 0 {
        println!(
            "  Reviews skipped: {} (film unmatched)",
            s.reviews_skipped_unmatched
        );
    }
    if s.reviews_skipped_over_limit > 0 {
        println!(
            "  Reviews skipped: {} (Trakt note limit reached)",
            s.reviews_skipped_over_limit
        );
    }
    if !s.unmatched.is_empty() {
        println!();
        println!("  Unmatched films ({}):", s.unmatched.len());
        for film in &s.unmatched {
            println!("    - {} ({})", film.title, film.year);
        }
    }
}

fn print_to_letterboxd_summary(s: &sync_to_letterboxd::SyncSummary, data_dir: &std::path::Path) {
    if s.dry_run {
        println!("[DRY RUN] Trakt \u{2192} Letterboxd export (no files written)");
    } else {
        println!("Trakt \u{2192} Letterboxd export complete");
    }
    println!();
    println!("  Diary rows:       {}", s.diary_rows);
    println!("  Ratings:          {}", s.ratings_in_diary);
    println!("  Reviews:          {}", s.reviews_in_diary);
    println!("  Watchlist rows:   {}", s.watchlist_rows);
    println!("  Already exported: {} skipped", s.skipped);
    if !s.dry_run {
        println!();
        println!(
            "  Diary CSV:     {}",
            data_dir.join("letterboxd-diary-import.csv").display()
        );
        println!(
            "  Watchlist CSV: {}",
            data_dir.join("letterboxd-watchlist-import.csv").display()
        );
        println!();
        println!("Next steps: Upload these files at https://letterboxd.com/import/ \u{2014} diary file first, then watchlist.");
    }
}

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
            SyncDirection::FromLetterboxd {
                path,
                dry_run,
                force,
            } => {
                let client = trakt_client::ReqwestClient::new(&cfg.trakt_client_id);
                let token = match auth::get_valid_token(
                    &client,
                    &cfg.trakt_client_id,
                    &cfg.trakt_client_secret,
                    &cfg.data_dir,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                };
                match sync_from_letterboxd::run(
                    &client,
                    &cfg.data_dir,
                    TRAKT_BASE_URL,
                    &token,
                    path,
                    *dry_run,
                    *force,
                ) {
                    Ok(s) => print_from_letterboxd_summary(&s),
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                }
            }
            SyncDirection::ToLetterboxd { dry_run, force } => {
                let client = trakt_client::ReqwestClient::new(&cfg.trakt_client_id);
                let token = match auth::get_valid_token(
                    &client,
                    &cfg.trakt_client_id,
                    &cfg.trakt_client_secret,
                    &cfg.data_dir,
                ) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                };
                match sync_to_letterboxd::run(
                    &client,
                    &cfg.data_dir,
                    TRAKT_BASE_URL,
                    &token,
                    *dry_run,
                    *force,
                ) {
                    Ok(s) => print_to_letterboxd_summary(&s, &cfg.data_dir),
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                }
            }
        },
    }
}
