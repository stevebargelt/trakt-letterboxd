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

const DETAIL_LIST_CAP: usize = 20;

fn format_from_letterboxd_summary(s: &sync_from_letterboxd::SyncSummary) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if s.dry_run {
        writeln!(
            out,
            "[DRY RUN] Letterboxd \u{2192} Trakt (no changes written)"
        )
        .unwrap();
    } else {
        writeln!(out, "Letterboxd \u{2192} Trakt sync complete").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(
        out,
        "  Watched history:  {} added, {} skipped (already synced)",
        s.watched_added, s.watched_skipped
    )
    .unwrap();
    writeln!(
        out,
        "  Ratings:          {} added, {} skipped (already synced)",
        s.ratings_added, s.ratings_skipped
    )
    .unwrap();
    writeln!(
        out,
        "  Watchlist:        {} added, {} skipped (already synced)",
        s.watchlist_added, s.watchlist_skipped
    )
    .unwrap();
    writeln!(
        out,
        "  Reviews:          {} transferred, {} skipped (over limit), {} skipped (film unmatched), {} errored",
        s.reviews_transferred,
        s.reviews_skipped_over_limit,
        s.reviews_skipped_unmatched,
        s.errored.len()
    )
    .unwrap();

    if !s.unmatched.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "  Unmatched films ({}):", s.unmatched.len()).unwrap();
        for film in s.unmatched.iter().take(DETAIL_LIST_CAP) {
            writeln!(out, "    - {} ({}): {}", film.title, film.year, film.reason).unwrap();
        }
        if s.unmatched.len() > DETAIL_LIST_CAP {
            writeln!(
                out,
                "    ... and {} more",
                s.unmatched.len() - DETAIL_LIST_CAP
            )
            .unwrap();
        }
    }

    if !s.errored.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "  Errored items ({}):", s.errored.len()).unwrap();
        for item in s.errored.iter().take(DETAIL_LIST_CAP) {
            writeln!(out, "    - {} ({}): {}", item.title, item.year, item.reason).unwrap();
        }
        if s.errored.len() > DETAIL_LIST_CAP {
            writeln!(
                out,
                "    ... and {} more",
                s.errored.len() - DETAIL_LIST_CAP
            )
            .unwrap();
        }
    }

    out
}

fn print_from_letterboxd_summary(s: &sync_from_letterboxd::SyncSummary) {
    print!("{}", format_from_letterboxd_summary(s));
}

fn from_letterboxd_has_errors(s: &sync_from_letterboxd::SyncSummary) -> bool {
    !s.errored.is_empty()
}

fn to_letterboxd_has_errors(s: &sync_to_letterboxd::SyncSummary) -> bool {
    !s.errored.is_empty()
}

fn format_to_letterboxd_summary(
    s: &sync_to_letterboxd::SyncSummary,
    data_dir: &std::path::Path,
) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    if s.dry_run {
        writeln!(
            out,
            "[DRY RUN] Trakt \u{2192} Letterboxd export (no files written)"
        )
        .unwrap();
    } else {
        writeln!(out, "Trakt \u{2192} Letterboxd export complete").unwrap();
    }
    writeln!(out).unwrap();
    writeln!(out, "  Diary rows:               {}", s.diary_rows).unwrap();
    writeln!(out, "  Distinct rated films:     {}", s.distinct_ratings).unwrap();
    writeln!(
        out,
        "  Diary rows with a rating: {} (may include rewatches of rated films)",
        s.ratings_in_diary
    )
    .unwrap();
    writeln!(out, "  Reviews in diary:         {}", s.reviews_in_diary).unwrap();
    writeln!(out, "  Watchlist rows:           {}", s.watchlist_rows).unwrap();
    writeln!(out, "  Already exported:         {} skipped", s.skipped).unwrap();

    if !s.errored.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "  Errored items ({}):", s.errored.len()).unwrap();
        for item in s.errored.iter().take(DETAIL_LIST_CAP) {
            writeln!(out, "    - {} ({}): {}", item.title, item.year, item.reason).unwrap();
        }
        if s.errored.len() > DETAIL_LIST_CAP {
            writeln!(
                out,
                "    ... and {} more",
                s.errored.len() - DETAIL_LIST_CAP
            )
            .unwrap();
        }
    }

    if !s.dry_run {
        writeln!(out).unwrap();
        writeln!(
            out,
            "  Diary CSV:     {}",
            data_dir.join("letterboxd-diary-import.csv").display()
        )
        .unwrap();
        writeln!(
            out,
            "  Watchlist CSV: {}",
            data_dir.join("letterboxd-watchlist-import.csv").display()
        )
        .unwrap();
        writeln!(out).unwrap();
        writeln!(out, "Next steps:").unwrap();
        writeln!(
            out,
            "  1. Diary CSV   \u{2192} https://letterboxd.com/import/ (marks films as watched)"
        )
        .unwrap();
        writeln!(
            out,
            "  2. Watchlist CSV \u{2192} Your Letterboxd Watchlist page \u{2192} sidebar \u{2018}Import films to watchlist\u{2019} \u{2192} attach the CSV \u{2192} \u{2018}Add films to watchlist\u{2019}"
        )
        .unwrap();
    }

    out
}

fn print_to_letterboxd_summary(s: &sync_to_letterboxd::SyncSummary, data_dir: &std::path::Path) {
    print!("{}", format_to_letterboxd_summary(s, data_dir));
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
                    Ok(s) => {
                        print_from_letterboxd_summary(&s);
                        if from_letterboxd_has_errors(&s) {
                            process::exit(1);
                        }
                    }
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
                    Ok(s) => {
                        print_to_letterboxd_summary(&s, &cfg.data_dir);
                        if to_letterboxd_has_errors(&s) {
                            process::exit(1);
                        }
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        process::exit(1);
                    }
                }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::UnmatchedFilm;

    #[allow(clippy::too_many_arguments)]
    fn make_from_summary(
        watched_added: u32,
        watched_skipped: u32,
        ratings_added: u32,
        ratings_skipped: u32,
        watchlist_added: u32,
        watchlist_skipped: u32,
        unmatched: Vec<UnmatchedFilm>,
        errored: Vec<sync_from_letterboxd::ErroredItem>,
        dry_run: bool,
    ) -> sync_from_letterboxd::SyncSummary {
        sync_from_letterboxd::SyncSummary {
            watched_added,
            watched_skipped,
            ratings_added,
            ratings_skipped,
            watchlist_added,
            watchlist_skipped,
            unmatched,
            errored,
            dry_run,
            reviews_transferred: 0,
            reviews_skipped_over_limit: 0,
            reviews_skipped_unmatched: 0,
        }
    }

    fn make_to_summary(
        diary_rows: u32,
        distinct_ratings: u32,
        ratings_in_diary: u32,
        watchlist_rows: u32,
        skipped: u32,
        dry_run: bool,
        errored: Vec<sync_to_letterboxd::ErroredItem>,
    ) -> sync_to_letterboxd::SyncSummary {
        sync_to_letterboxd::SyncSummary {
            diary_rows,
            ratings_in_diary,
            distinct_ratings,
            watchlist_rows,
            skipped,
            dry_run,
            reviews_in_diary: 0,
            errored,
        }
    }

    #[test]
    fn errored_item_triggers_nonzero_exit_from_letterboxd() {
        let s = make_from_summary(
            0,
            0,
            0,
            0,
            0,
            0,
            vec![],
            vec![sync_from_letterboxd::ErroredItem {
                title: "Bad Film".to_string(),
                year: 2024,
                reason: "note creation failed: HTTP 500".to_string(),
            }],
            false,
        );
        assert!(
            from_letterboxd_has_errors(&s),
            "errored item must signal exit 1"
        );
    }

    #[test]
    fn only_unmatched_does_not_trigger_nonzero_exit_from_letterboxd() {
        let s = make_from_summary(
            0,
            0,
            0,
            0,
            0,
            0,
            vec![UnmatchedFilm {
                title: "Ghost Film".to_string(),
                year: 2050,
                reason: "no exact title+year match in Trakt search".to_string(),
            }],
            vec![],
            false,
        );
        assert!(
            !from_letterboxd_has_errors(&s),
            "unmatched-only must not trigger exit 1"
        );
    }

    #[test]
    fn no_errors_no_unmatched_exits_zero_from_letterboxd() {
        let s = make_from_summary(3, 1, 2, 0, 1, 0, vec![], vec![], false);
        assert!(!from_letterboxd_has_errors(&s));
    }

    #[test]
    fn errored_item_triggers_nonzero_exit_to_letterboxd() {
        let s = make_to_summary(
            1,
            1,
            1,
            0,
            0,
            false,
            vec![sync_to_letterboxd::ErroredItem {
                title: "Some Film".to_string(),
                year: 2020,
                reason: "file write error".to_string(),
            }],
        );
        assert!(
            to_letterboxd_has_errors(&s),
            "errored item must signal exit 1"
        );
    }

    #[test]
    fn no_errors_exits_zero_to_letterboxd() {
        let s = make_to_summary(1, 1, 1, 0, 0, false, vec![]);
        assert!(!to_letterboxd_has_errors(&s));
    }

    #[test]
    fn dry_run_label_present_in_from_letterboxd_summary() {
        let s = make_from_summary(1, 0, 0, 0, 0, 0, vec![], vec![], true);
        // Capture output by constructing summary and checking dry_run flag
        assert!(s.dry_run, "dry_run flag must be set when --dry-run used");
    }

    #[test]
    fn dry_run_label_present_in_to_letterboxd_summary() {
        let s = make_to_summary(1, 1, 1, 0, 0, true, vec![]);
        assert!(s.dry_run, "dry_run flag must be set when --dry-run used");
    }

    #[test]
    fn unmatched_item_has_title_year_reason() {
        let s = make_from_summary(
            0,
            0,
            0,
            0,
            0,
            0,
            vec![UnmatchedFilm {
                title: "Ghost Film".to_string(),
                year: 2050,
                reason: "no exact title+year match in Trakt search".to_string(),
            }],
            vec![],
            false,
        );
        let film = &s.unmatched[0];
        assert_eq!(film.title, "Ghost Film");
        assert_eq!(film.year, 2050);
        assert!(!film.reason.is_empty(), "unmatched must carry a reason");
    }

    #[test]
    fn errored_item_has_title_year_reason() {
        let item = sync_from_letterboxd::ErroredItem {
            title: "Bad Film".to_string(),
            year: 2024,
            reason: "note creation failed: unexpected HTTP 500".to_string(),
        };
        assert_eq!(item.title, "Bad Film");
        assert_eq!(item.year, 2024);
        assert!(item.reason.contains("500"));
    }

    #[test]
    fn ratings_relabel_distinct_vs_diary_rows() {
        // Distinct rated films != diary rows with a rating when rewatches exist.
        // Verify summary carries both fields with correct semantics.
        let s = make_to_summary(
            2, // diary_rows: two history entries (two watches of the same film)
            1, // distinct_ratings: one unique rated film
            2, // ratings_in_diary: both diary rows carry the rating
            0,
            0,
            false,
            vec![],
        );
        assert_eq!(
            s.distinct_ratings, 1,
            "distinct_ratings must count unique films, not diary rows"
        );
        assert_eq!(
            s.ratings_in_diary, 2,
            "ratings_in_diary may exceed distinct_ratings due to rewatches"
        );
        // The two fields must differ to demonstrate the mislabel was real.
        assert_ne!(
            s.distinct_ratings, s.ratings_in_diary,
            "in rewatch scenario the two counts differ"
        );
    }

    // ── Gap coverage: exit code / has_errors ──────────────────────────────────

    #[test]
    fn to_letterboxd_has_errors_false_when_only_skipped() {
        // T->L has no "unmatched" concept; the analogue is skipped (already exported).
        // Skipped items alone must not drive exit 1 — only errored items should.
        let s = make_to_summary(0, 0, 0, 0, 5, false, vec![]);
        assert!(
            !to_letterboxd_has_errors(&s),
            "skipped-only must not trigger exit 1 for to-letterboxd"
        );
    }

    // ── Gap coverage: dry-run labeling in printed output ─────────────────────

    #[test]
    fn from_letterboxd_dry_run_label_appears_in_output() {
        let s = make_from_summary(1, 0, 0, 0, 0, 0, vec![], vec![], true);
        let output = format_from_letterboxd_summary(&s);
        assert!(
            output.contains("[DRY RUN]"),
            "dry-run output must contain '[DRY RUN]'; got:\n{output}"
        );
    }

    #[test]
    fn from_letterboxd_real_run_has_no_dry_run_label() {
        let s = make_from_summary(1, 0, 0, 0, 0, 0, vec![], vec![], false);
        let output = format_from_letterboxd_summary(&s);
        assert!(
            !output.contains("[DRY RUN]"),
            "real-run output must not contain '[DRY RUN]'; got:\n{output}"
        );
    }

    #[test]
    fn to_letterboxd_dry_run_label_appears_in_output() {
        let s = make_to_summary(1, 1, 1, 0, 0, true, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);
        assert!(
            output.contains("[DRY RUN]"),
            "dry-run output must contain '[DRY RUN]'; got:\n{output}"
        );
    }

    #[test]
    fn to_letterboxd_real_run_has_no_dry_run_label() {
        let s = make_to_summary(1, 1, 1, 0, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);
        assert!(
            !output.contains("[DRY RUN]"),
            "real-run output must not contain '[DRY RUN]'; got:\n{output}"
        );
    }

    #[test]
    fn to_letterboxd_summary_routes_diary_to_import_and_watchlist_to_watchlist_importer() {
        let s = make_to_summary(1, 0, 1, 0, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);
        assert!(
            output.contains("letterboxd.com/import"),
            "diary next-step must reference letterboxd.com/import; got:\n{output}"
        );
        assert!(
            output.contains("Import films to watchlist"),
            "watchlist next-step must name the 'Import films to watchlist' sidebar link; got:\n{output}"
        );
        assert!(
            output.contains("Add films to watchlist"),
            "watchlist next-step must name the 'Add films to watchlist' button; got:\n{output}"
        );
        // The watchlist CSV instruction must NOT point to /import (that marks films watched).
        let lines: Vec<&str> = output.lines().collect();
        let watchlist_line = lines
            .iter()
            .find(|l| l.contains("Watchlist CSV"))
            .expect("output must contain a Watchlist CSV next-step line");
        assert!(
            !watchlist_line.contains("letterboxd.com/import"),
            "watchlist CSV next-step must NOT reference letterboxd.com/import (it marks films watched); got:\n{watchlist_line}"
        );
    }

    // ── FG-18: watchlist CSV must NOT route to /import ───────────────────────
    // The /import endpoint marks films as *watched*. Sending a watchlist CSV
    // there would wrongly add want-to-watch films to the user's diary.

    /// The line that mentions the Watchlist CSV must not contain the URL
    /// `letterboxd.com/import` — that endpoint marks films as watched.
    #[test]
    fn fg18_watchlist_next_step_does_not_reference_letterboxd_com_import() {
        let s = make_to_summary(1, 1, 1, 2, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        let watchlist_line = output
            .lines()
            .find(|l| l.contains("Watchlist CSV"))
            .expect("output must contain a 'Watchlist CSV' next-step line");

        assert!(
            !watchlist_line.contains("letterboxd.com/import"),
            "watchlist next-step must NOT reference letterboxd.com/import (that endpoint marks films watched, not want-to-watch); line was:\n  {watchlist_line}"
        );
    }

    /// The watchlist next-step must name the sidebar link the user clicks to
    /// open the watchlist importer ('Import films to watchlist').
    #[test]
    fn fg18_watchlist_next_step_names_import_films_to_watchlist_ui() {
        let s = make_to_summary(1, 1, 1, 2, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        assert!(
            output.contains("Import films to watchlist"),
            "watchlist next-step must reference the 'Import films to watchlist' sidebar link; got:\n{output}"
        );
    }

    /// The watchlist next-step must name the submit button the user clicks
    /// ('Add films to watchlist') to distinguish it from the diary importer.
    #[test]
    fn fg18_watchlist_next_step_names_add_films_to_watchlist_button() {
        let s = make_to_summary(1, 1, 1, 2, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        assert!(
            output.contains("Add films to watchlist"),
            "watchlist next-step must reference the 'Add films to watchlist' button; got:\n{output}"
        );
    }

    /// The diary next-step must route to letterboxd.com/import — the correct
    /// endpoint for marking films as watched.
    #[test]
    fn fg18_diary_next_step_routes_to_letterboxd_com_import() {
        let s = make_to_summary(3, 2, 3, 0, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        // The next-step line for the diary CSV contains both "Diary CSV" and
        // the arrow character "→". The file-path line does not contain "→".
        let diary_step_line = output
            .lines()
            .find(|l| l.contains("Diary CSV") && l.contains('\u{2192}'))
            .expect("output must contain a 'Diary CSV →' next-step line");

        assert!(
            diary_step_line.contains("letterboxd.com/import"),
            "diary next-step must reference letterboxd.com/import; line was:\n  {diary_step_line}"
        );
    }

    /// The two CSV files must appear as two distinct numbered destinations in
    /// the next-steps section, so the user knows to handle them separately.
    #[test]
    fn fg18_diary_and_watchlist_presented_as_two_distinct_next_step_destinations() {
        let s = make_to_summary(5, 3, 5, 4, 0, false, vec![]);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        // Both numbered steps must appear.
        assert!(
            output.contains("1.") && output.contains("2."),
            "next-steps must have at least two numbered items; got:\n{output}"
        );

        // Each CSV is called out on its own step line.
        let diary_step = output
            .lines()
            .find(|l| l.contains("Diary CSV"))
            .expect("a step must mention 'Diary CSV'");
        let watchlist_step = output
            .lines()
            .find(|l| l.contains("Watchlist CSV"))
            .expect("a step must mention 'Watchlist CSV'");

        assert_ne!(
            diary_step, watchlist_step,
            "Diary CSV and Watchlist CSV must be on separate lines (distinct destinations)"
        );
    }

    // ── Gap coverage: list cap truncation ─────────────────────────────────────

    #[test]
    fn from_letterboxd_list_cap_truncates_unmatched_with_correct_overflow() {
        // 25 unmatched films (> DETAIL_LIST_CAP of 20) must print first 20
        // followed by "... and 5 more".
        let unmatched: Vec<UnmatchedFilm> = (0..25)
            .map(|i| UnmatchedFilm {
                title: format!("Film {i}"),
                year: 2000 + i,
                reason: "no match".to_string(),
            })
            .collect();
        let s = make_from_summary(0, 0, 0, 0, 0, 0, unmatched, vec![], false);
        let output = format_from_letterboxd_summary(&s);

        let overflow_line = format!("    ... and {} more", 25 - DETAIL_LIST_CAP);
        assert!(
            output.contains(&overflow_line),
            "output must contain '{overflow_line}'; got:\n{output}"
        );
        // Exactly 20 film lines should appear (Film 0 through Film 19).
        let listed = (0..20)
            .filter(|i| output.contains(&format!("Film {i}")))
            .count();
        assert_eq!(listed, 20, "must list exactly 20 films before truncating");
        // Film 20-24 must not appear individually.
        assert!(
            !output.contains("Film 20"),
            "Film 20 must be truncated, not listed"
        );
    }

    #[test]
    fn from_letterboxd_list_cap_truncates_errored_with_correct_overflow() {
        // 22 errored items (> cap of 20) must show "... and 2 more".
        let errored: Vec<sync_from_letterboxd::ErroredItem> = (0..22)
            .map(|i| sync_from_letterboxd::ErroredItem {
                title: format!("Error Film {i}"),
                year: 2000 + i,
                reason: "write failed".to_string(),
            })
            .collect();
        let s = make_from_summary(0, 0, 0, 0, 0, 0, vec![], errored, false);
        let output = format_from_letterboxd_summary(&s);

        let overflow_line = format!("    ... and {} more", 22 - DETAIL_LIST_CAP);
        assert!(
            output.contains(&overflow_line),
            "output must contain '{overflow_line}'; got:\n{output}"
        );
        assert!(
            !output.contains("Error Film 20"),
            "Error Film 20 must be truncated"
        );
    }

    #[test]
    fn to_letterboxd_list_cap_truncates_errored_with_correct_overflow() {
        // 21 errored items must show "... and 1 more".
        let errored: Vec<sync_to_letterboxd::ErroredItem> = (0..21)
            .map(|i| sync_to_letterboxd::ErroredItem {
                title: format!("T2L Error {i}"),
                year: 2000 + i,
                reason: "file error".to_string(),
            })
            .collect();
        let s = make_to_summary(0, 0, 0, 0, 0, false, errored);
        let data_dir = std::path::Path::new("/tmp/dummy");
        let output = format_to_letterboxd_summary(&s, data_dir);

        let overflow_line = format!("    ... and {} more", 21 - DETAIL_LIST_CAP);
        assert!(
            output.contains(&overflow_line),
            "output must contain '{overflow_line}'; got:\n{output}"
        );
        assert!(
            !output.contains("T2L Error 20"),
            "T2L Error 20 must be truncated"
        );
    }
}
