use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;

use crate::{
    letterboxd_export::LetterboxdExport,
    letterboxd_import::{write_diary_csv, write_watchlist_csv},
    sync_state::{Direction, ItemRef, ItemType, SyncKey, SyncState},
    trakt_client::TraktHttpClient,
    trakt_notes,
    trakt_read::{
        fetch_ratings, fetch_watched_history, fetch_watchlist, WatchedMovie, WatchlistMovie,
    },
};

/// Films per calendar day at or above this count are considered a bulk-add event.
/// A typical film marathon tops out around 8–9 films; the real-world oracle cluster
/// was 86 films on a single day. 10 sits safely between casual use and bulk noise.
pub const BULK_DATE_THRESHOLD: usize = 10;

pub struct ErroredItem {
    pub title: String,
    pub year: u32,
    pub reason: String,
}

pub struct SyncSummary {
    pub diary_rows: u32,
    pub ratings_in_diary: u32,
    pub distinct_ratings: u32,
    pub watchlist_rows: u32,
    pub skipped: u32,
    pub dry_run: bool,
    pub reviews_in_diary: u32,
    pub errored: Vec<ErroredItem>,
    /// Films date-enriched from Trakt (was dateless on LB, clean watch day).
    pub enriched: u32,
    /// Films already dated on LB — skipped to avoid duplicate diary entries.
    pub skipped_existing: u32,
    /// Films dateless on LB on a bulk-date day — skipped to avoid planting junk dates.
    pub skipped_bulk: u32,
    /// Net-new films on a bulk-date day — emitted with a blank WatchedDate.
    pub net_new_bulk: u32,
}

fn truncate_date(ts: &str) -> &str {
    ts.split('T').next().unwrap_or(ts)
}

fn item_ref(tmdb_id: Option<u64>, title: &str, year: Option<u32>) -> ItemRef {
    match tmdb_id {
        Some(id) => ItemRef::Tmdb(id),
        None => ItemRef::TitleYear(title.to_owned(), year.unwrap_or(0) as u16),
    }
}

fn watched_key(entry: &WatchedMovie) -> SyncKey {
    let date = truncate_date(&entry.watched_at).to_owned();
    SyncKey::new(
        Direction::TraktToLetterboxd,
        ItemType::Watched,
        item_ref(entry.movie.tmdb_id, &entry.movie.title, entry.movie.year),
        date,
    )
}

fn watchlist_entry_key(entry: &WatchlistMovie) -> SyncKey {
    SyncKey::new(
        Direction::TraktToLetterboxd,
        ItemType::Watchlist,
        item_ref(entry.movie.tmdb_id, &entry.movie.title, entry.movie.year),
        "",
    )
}

/// Normalise a title for fuzzy matching: lowercase, strip leading articles.
pub fn normalize_title(s: &str) -> String {
    let lower = s.to_lowercase();
    for prefix in ["the ", "a ", "an "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    lower
}

#[derive(Clone, Copy, PartialEq)]
enum LbStatus {
    Dated,
    Dateless,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    client: &dyn TraktHttpClient,
    data_dir: &Path,
    base_url: &str,
    access_token: &str,
    dry_run: bool,
    force: bool,
    lb_export: Option<&Path>,
    include_ratings: bool,
) -> Result<SyncSummary, String> {
    let mut history = fetch_watched_history(client, base_url, access_token)?;
    let ratings = fetch_ratings(client, base_url, access_token)?;
    let distinct_ratings = ratings.len() as u32;
    let mut watchlist = fetch_watchlist(client, base_url, access_token)?;

    let mut state = SyncState::load(data_dir);
    if force {
        state.clear_direction(&Direction::TraktToLetterboxd);
    }

    let mut skipped = 0u32;

    // Precompute bulk days from the FULL history before SyncState filtering.
    let bulk_dates: HashSet<String> = if lb_export.is_some() {
        let mut day_counts: HashMap<String, usize> = HashMap::new();
        for entry in &history {
            *day_counts
                .entry(truncate_date(&entry.watched_at).to_owned())
                .or_insert(0) += 1;
        }
        day_counts
            .into_iter()
            .filter(|(_, count)| *count >= BULK_DATE_THRESHOLD)
            .map(|(day, _)| day)
            .collect()
    } else {
        HashSet::new()
    };

    history.retain(|e| {
        if state.contains(&watched_key(e)) {
            skipped += 1;
            false
        } else {
            true
        }
    });

    watchlist.retain(|e| {
        if state.contains(&watchlist_entry_key(e)) {
            skipped += 1;
            false
        } else {
            true
        }
    });

    let notes = trakt_notes::fetch_movie_notes(client, base_url, access_token);

    let rating_map: HashMap<u64, u8> = ratings
        .iter()
        .filter_map(|r| r.movie.tmdb_id.map(|id| (id, r.rating)))
        .collect();

    // Build lb_map: (normalized_title, year) -> LbStatus
    let lb_map: HashMap<(String, u32), LbStatus> = if let Some(path) = lb_export {
        let export = LetterboxdExport::load(path)?;
        let mut map: HashMap<(String, u32), LbStatus> = HashMap::new();

        // Diary entries: non-empty watched_date = Dated, empty = Dateless. Dated wins.
        for entry in &export.diary {
            let key = (normalize_title(&entry.name), entry.year);
            let status = if entry.watched_date.is_empty() {
                LbStatus::Dateless
            } else {
                LbStatus::Dated
            };
            let existing = map.entry(key).or_insert(status);
            if status == LbStatus::Dated {
                *existing = LbStatus::Dated;
            }
        }

        // Watched.csv entries: always Dateless (do not overwrite Dated from diary).
        for entry in &export.watched {
            let key = (normalize_title(&entry.name), entry.year);
            map.entry(key).or_insert(LbStatus::Dateless);
        }

        map
    } else {
        HashMap::new()
    };

    // Classify each history entry into a bucket.
    let mut enriched = 0u32;
    let mut skipped_existing = 0u32;
    let mut skipped_bulk = 0u32;
    let mut net_new_bulk = 0u32;

    let entries: Vec<(&WatchedMovie, Option<&str>)> = if lb_export.is_some() {
        let mut out: Vec<(&WatchedMovie, Option<&str>)> = Vec::new();
        for entry in &history {
            let day = truncate_date(&entry.watched_at);
            let is_bulk = bulk_dates.contains(day);
            let key = (
                normalize_title(&entry.movie.title),
                entry.movie.year.unwrap_or(0),
            );
            match lb_map.get(&key) {
                Some(LbStatus::Dated) => {
                    skipped_existing += 1;
                    // no diary row
                }
                Some(LbStatus::Dateless) if is_bulk => {
                    skipped_bulk += 1;
                    // no diary row — would plant a junk date on an already-watched film
                }
                Some(LbStatus::Dateless) => {
                    // enrich: emit with the Trakt watch date
                    enriched += 1;
                    out.push((entry, None));
                }
                None if is_bulk => {
                    // net-new on bulk day: mark watched, blank date to avoid fake diary entry
                    net_new_bulk += 1;
                    out.push((entry, Some("")));
                }
                _ => {
                    // net-new on clean day: emit with Trakt date
                    out.push((entry, None));
                }
            }
        }
        out
    } else {
        // No LB export: emit all SyncState-filtered entries as-is (today's behaviour).
        history.iter().map(|m| (m, None)).collect()
    };

    let diary_rows = entries.len() as u32;

    let ratings_in_diary = entries
        .iter()
        .filter(|(entry, _)| {
            entry
                .movie
                .tmdb_id
                .map(|id| rating_map.contains_key(&id))
                .unwrap_or(false)
        })
        .count() as u32;

    let reviews_in_diary = entries
        .iter()
        .filter(|(entry, _)| {
            entry
                .movie
                .tmdb_id
                .map(|id| notes.contains_key(&id))
                .unwrap_or(false)
        })
        .count() as u32;

    let watchlist_rows = watchlist.len() as u32;

    if !dry_run {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;

        let diary_file = std::fs::File::create(data_dir.join("letterboxd-diary-import.csv"))
            .map_err(|e| format!("failed to create diary CSV: {e}"))?;
        write_diary_csv(
            io::BufWriter::new(diary_file),
            &entries,
            &ratings,
            &notes,
            include_ratings,
        )?;

        let watchlist_file =
            std::fs::File::create(data_dir.join("letterboxd-watchlist-import.csv"))
                .map_err(|e| format!("failed to create watchlist CSV: {e}"))?;
        write_watchlist_csv(io::BufWriter::new(watchlist_file), &watchlist)?;

        for (entry, _) in &entries {
            state.mark(watched_key(entry));
        }
        for entry in &watchlist {
            state.mark(watchlist_entry_key(entry));
        }
        state.save(data_dir)?;
    }

    Ok(SyncSummary {
        diary_rows,
        ratings_in_diary,
        distinct_ratings,
        watchlist_rows,
        skipped,
        dry_run,
        reviews_in_diary,
        errored: Vec::new(),
        enriched,
        skipped_existing,
        skipped_bulk,
        net_new_bulk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sync_state::{Direction, ItemRef, ItemType, SyncKey, SyncState},
        trakt_client::{HttpResponse, TraktHttpClient},
    };
    use std::collections::{HashMap, VecDeque};
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[allow(clippy::type_complexity)]
    struct MockClient {
        responses: Mutex<VecDeque<(u16, String, HashMap<String, String>)>>,
    }

    impl MockClient {
        fn new(responses: Vec<(u16, String, HashMap<String, String>)>) -> Self {
            MockClient {
                responses: Mutex::new(responses.into()),
            }
        }
    }

    impl TraktHttpClient for MockClient {
        fn post_json(&self, _url: &str, _body: &str) -> Result<HttpResponse, String> {
            unreachable!("sync_to_letterboxd tests do not call post_json")
        }

        fn post_json_auth(
            &self,
            _url: &str,
            _body: &str,
            _access_token: &str,
        ) -> Result<HttpResponse, String> {
            unreachable!("sync_to_letterboxd tests do not call post_json_auth")
        }

        fn get(&self, _url: &str, _access_token: &str) -> Result<HttpResponse, String> {
            let mut q = self.responses.lock().unwrap();
            let (status, body, headers) = q
                .pop_front()
                .ok_or_else(|| "no more mock responses".to_string())?;
            Ok(HttpResponse {
                status,
                body,
                headers,
            })
        }
    }

    fn page_headers(count: u32) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("x-pagination-page-count".to_string(), count.to_string());
        h
    }

    fn history_json(entries: &[(&str, u32, u64, &str)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(title, year, tmdb, watched_at)| {
                format!(
                    r#"{{"watched_at":"{watched_at}","movie":{{"title":"{title}","year":{year},"ids":{{"trakt":1,"slug":"slug","imdb":"tt1","tmdb":{tmdb}}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn ratings_json(entries: &[(&str, u32, u64, u8)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(title, year, tmdb, rating)| {
                format!(
                    r#"{{"rated_at":"2024-01-01T00:00:00.000Z","rating":{rating},"movie":{{"title":"{title}","year":{year},"ids":{{"trakt":1,"slug":"slug","imdb":"tt1","tmdb":{tmdb}}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn watchlist_json(entries: &[(&str, u32, u64)]) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(title, year, tmdb)| {
                format!(
                    r#"{{"listed_at":"2024-01-01T00:00:00.000Z","movie":{{"title":"{title}","year":{year},"ids":{{"trakt":1,"slug":"slug","imdb":"tt1","tmdb":{tmdb}}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    fn standard_responses(
        history: &[(&str, u32, u64, &str)],
        ratings: &[(&str, u32, u64, u8)],
        watchlist: &[(&str, u32, u64)],
    ) -> Vec<(u16, String, HashMap<String, String>)> {
        vec![
            (200, history_json(history), page_headers(1)),
            (200, ratings_json(ratings), page_headers(1)),
            (200, watchlist_json(watchlist), page_headers(1)),
        ]
    }

    /// Write a minimal Letterboxd diary CSV to a temp dir and return the dir.
    /// `entries` is (name, year, watched_date) — empty watched_date = dateless.
    fn make_lb_export_dir(entries: &[(&str, u32, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        let mut f = std::fs::File::create(dir.path().join("diary.csv")).unwrap();
        writeln!(
            f,
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date"
        )
        .unwrap();
        for (name, year, watched_date) in entries {
            writeln!(
                f,
                "2024-01-01,{name},{year},https://letterboxd.com/film/slug/,,,,{watched_date}"
            )
            .unwrap();
        }
        dir
    }

    #[test]
    fn dry_run_writes_no_files_and_reports_correct_counts() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[("Dune", 2021, 438631)],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            true,
            false,
            None,
            true,
        )
        .unwrap();

        assert!(summary.dry_run);
        assert_eq!(summary.diary_rows, 1);
        assert_eq!(summary.ratings_in_diary, 1);
        assert_eq!(summary.watchlist_rows, 1);
        assert_eq!(summary.skipped, 0);

        assert!(
            !data_dir.path().join("letterboxd-diary-import.csv").exists(),
            "dry run must not write diary CSV"
        );
        assert!(
            !data_dir
                .path()
                .join("letterboxd-watchlist-import.csv")
                .exists(),
            "dry run must not write watchlist CSV"
        );
        assert!(
            !data_dir.path().join("sync_state.json").exists(),
            "dry run must not update sync state"
        );
    }

    #[test]
    fn real_run_writes_both_csvs_with_expected_headers_and_rows() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[("Dune", 2021, 438631)],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert!(!summary.dry_run);
        assert_eq!(summary.diary_rows, 1);
        assert_eq!(summary.watchlist_rows, 1);

        let diary_path = data_dir.path().join("letterboxd-diary-import.csv");
        let watchlist_path = data_dir.path().join("letterboxd-watchlist-import.csv");

        assert!(diary_path.exists(), "diary CSV must be written");
        assert!(watchlist_path.exists(), "watchlist CSV must be written");

        let diary_content = std::fs::read_to_string(&diary_path).unwrap();
        let diary_lines: Vec<&str> = diary_content.lines().collect();
        assert_eq!(
            diary_lines[0],
            "Title,Year,tmdbID,WatchedDate,Rating,Rewatch,Tags,Review"
        );
        assert!(diary_lines[1].contains("The Matrix"));
        assert!(diary_lines[1].contains("2024-01-15"));

        let watchlist_content = std::fs::read_to_string(&watchlist_path).unwrap();
        let watchlist_lines: Vec<&str> = watchlist_content.lines().collect();
        assert_eq!(watchlist_lines[0], "Title,Year,tmdbID");
        assert!(watchlist_lines[1].contains("Dune"));
    }

    #[test]
    fn already_exported_watched_items_are_skipped() {
        let data_dir = TempDir::new().unwrap();

        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "2024-01-15",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.diary_rows, 0);
    }

    #[test]
    fn already_exported_watchlist_items_are_skipped() {
        let data_dir = TempDir::new().unwrap();

        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watchlist,
            ItemRef::Tmdb(438631),
            "",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(standard_responses(&[], &[], &[("Dune", 2021, 438631)]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.watchlist_rows, 0);
    }

    #[test]
    fn force_re_exports_already_exported_items() {
        let data_dir = TempDir::new().unwrap();

        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "2024-01-15",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            true, // force
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.skipped, 0);
        assert_eq!(summary.diary_rows, 1);
    }

    #[test]
    fn state_saved_after_real_run_prevents_re_export() {
        let data_dir = TempDir::new().unwrap();

        let client1 = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));
        let s1 = run(
            &client1,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();
        assert_eq!(s1.diary_rows, 1);

        let client2 = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));
        let s2 = run(
            &client2,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(s2.skipped, 1);
        assert_eq!(s2.diary_rows, 0);
    }

    #[test]
    fn empty_trakt_data_produces_header_only_csvs() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(&[], &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.diary_rows, 0);
        assert_eq!(summary.watchlist_rows, 0);
        assert_eq!(summary.skipped, 0);

        let diary_content =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let diary_lines: Vec<&str> = diary_content.lines().collect();
        assert_eq!(
            diary_lines.len(),
            1,
            "empty history produces header-only diary CSV"
        );
        assert_eq!(
            diary_lines[0],
            "Title,Year,tmdbID,WatchedDate,Rating,Rewatch,Tags,Review"
        );
    }

    // GAP 1: Cross-run idempotency — second run with identical Trakt data writes header-only CSVs.
    #[test]
    fn cross_run_second_export_produces_header_only_csvs() {
        let data_dir = TempDir::new().unwrap();

        let client1 = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[("Dune", 2021, 438631)],
        ));
        let s1 = run(
            &client1,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();
        assert_eq!(s1.diary_rows, 1);
        assert_eq!(s1.watchlist_rows, 1);

        let client2 = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[("Dune", 2021, 438631)],
        ));
        let s2 = run(
            &client2,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();
        assert_eq!(s2.skipped, 2, "both items must be skipped on second run");
        assert_eq!(s2.diary_rows, 0);
        assert_eq!(s2.watchlist_rows, 0);

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let diary_lines: Vec<&str> = diary.lines().collect();
        assert_eq!(
            diary_lines.len(),
            1,
            "second run must not duplicate diary rows; got: {:?}",
            diary_lines
        );
        assert_eq!(
            diary_lines[0],
            "Title,Year,tmdbID,WatchedDate,Rating,Rewatch,Tags,Review"
        );

        let watchlist =
            std::fs::read_to_string(data_dir.path().join("letterboxd-watchlist-import.csv"))
                .unwrap();
        let watchlist_lines: Vec<&str> = watchlist.lines().collect();
        assert_eq!(
            watchlist_lines.len(),
            1,
            "second run must not duplicate watchlist rows; got: {:?}",
            watchlist_lines
        );
        assert_eq!(watchlist_lines[0], "Title,Year,tmdbID");
    }

    // GAP 2: Exact diary row content.
    #[test]
    fn csv_diary_row_has_exact_content_with_rating_conversion() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[("Dune", 2021, 438631)],
        ));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let diary_lines: Vec<&str> = diary.lines().collect();
        assert_eq!(diary_lines.len(), 2, "expected header + one data row");
        assert_eq!(
            diary_lines[1], "The Matrix,1999,603,2024-01-15,4.0,No,,",
            "diary row must encode Trakt rating 8 as Letterboxd 4.0 with all fields present"
        );

        let watchlist =
            std::fs::read_to_string(data_dir.path().join("letterboxd-watchlist-import.csv"))
                .unwrap();
        let watchlist_lines: Vec<&str> = watchlist.lines().collect();
        assert_eq!(
            watchlist_lines.len(),
            2,
            "watchlist CSV must have header + one watchlist row only"
        );
        assert_eq!(watchlist_lines[0], "Title,Year,tmdbID");
        assert_eq!(watchlist_lines[1], "Dune,2021,438631");
    }

    // GAP 4: Empty account — watchlist CSV must also be header-only.
    #[test]
    fn empty_account_watchlist_csv_is_also_header_only() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(&[], &[], &[]));
        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.watchlist_rows, 0);

        let watchlist =
            std::fs::read_to_string(data_dir.path().join("letterboxd-watchlist-import.csv"))
                .unwrap();
        let watchlist_lines: Vec<&str> = watchlist.lines().collect();
        assert_eq!(
            watchlist_lines.len(),
            1,
            "empty account must produce header-only watchlist CSV"
        );
        assert_eq!(watchlist_lines[0], "Title,Year,tmdbID");
    }

    // GAP 5: A watched film with no tmdb_id must appear in the diary CSV.
    #[test]
    fn watched_film_without_tmdb_id_appears_in_diary_csv() {
        let data_dir = TempDir::new().unwrap();

        let history_no_tmdb = r#"[{"watched_at":"2024-05-10T00:00:00.000Z","movie":{"title":"Obscure Film","year":1985,"ids":{"trakt":99,"slug":"obscure","imdb":"tt0000000","tmdb":null}}}]"#;
        let responses = vec![
            (200, history_no_tmdb.to_string(), page_headers(1)),
            (200, "[]".to_string(), page_headers(1)),
            (200, "[]".to_string(), page_headers(1)),
        ];
        let client = MockClient::new(responses);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            summary.diary_rows, 1,
            "film without tmdb_id must be counted in diary rows — no data loss"
        );

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let diary_lines: Vec<&str> = diary.lines().collect();
        assert_eq!(diary_lines.len(), 2, "expected header + one data row");
        assert!(
            diary_lines[1].starts_with("Obscure Film,1985,,"),
            "watched film without tmdb_id must appear with Title+Year; tmdbID column empty: got {}",
            diary_lines[1]
        );
    }

    // GAP 6: CSV output files at expected paths.
    #[test]
    fn run_writes_csvs_at_expected_filename_paths() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("Inception", 2010, 27205, "2024-03-01T00:00:00.000Z")],
            &[],
            &[("Blade Runner", 1982, 78)],
        ));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert!(
            data_dir.path().join("letterboxd-diary-import.csv").exists(),
            "diary CSV must be at <data_dir>/letterboxd-diary-import.csv"
        );
        assert!(
            data_dir
                .path()
                .join("letterboxd-watchlist-import.csv")
                .exists(),
            "watchlist CSV must be at <data_dir>/letterboxd-watchlist-import.csv"
        );
    }

    fn note_json(tmdb: u64, note: &str) -> String {
        format!(
            r#"[{{"id":1,"note":"{note}","spoiler":false,"privacy":"private","likes":0,"replies":0,"attached_to":{{"type":"movie","id":1}},"movie":{{"title":"Film","year":2024,"ids":{{"trakt":1,"slug":"film","imdb":"tt1","tmdb":{tmdb}}}}},"created_at":"2024-01-01T00:00:00.000Z","updated_at":"2024-01-01T00:00:00.000Z"}}]"#
        )
    }

    #[test]
    fn t2l_reads_trakt_note_into_review_column() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(vec![
            (
                200,
                history_json(&[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")]),
                page_headers(1),
            ),
            (200, ratings_json(&[]), page_headers(1)),
            (200, watchlist_json(&[]), page_headers(1)),
            (
                200,
                note_json(603, "Absolutely brilliant."),
                page_headers(1),
            ),
        ]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            summary.reviews_in_diary, 1,
            "one note should populate Review column"
        );

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let lines: Vec<&str> = diary.lines().collect();
        assert_eq!(lines.len(), 2, "header + one data row");
        assert!(
            lines[1].contains("Absolutely brilliant."),
            "Review column must contain the note text: {}",
            lines[1]
        );
    }

    #[test]
    fn t2l_dry_run_reports_reviews_in_diary_without_writing() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(vec![
            (
                200,
                history_json(&[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")]),
                page_headers(1),
            ),
            (200, ratings_json(&[]), page_headers(1)),
            (200, watchlist_json(&[]), page_headers(1)),
            (200, note_json(603, "Great film."), page_headers(1)),
        ]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            true,
            false,
            None,
            true,
        )
        .unwrap();

        assert!(summary.dry_run);
        assert_eq!(
            summary.reviews_in_diary, 1,
            "dry run must still report review count"
        );
        assert!(
            !data_dir.path().join("letterboxd-diary-import.csv").exists(),
            "dry run must not write files"
        );
    }

    #[test]
    fn t2l_no_notes_reviews_in_diary_is_zero() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            summary.reviews_in_diary, 0,
            "no notes available → zero reviews in diary"
        );
    }

    #[test]
    fn distinct_ratings_matches_trakt_ratings_count() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(vec![
            (
                200,
                history_json(&[
                    ("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z"),
                    ("The Matrix", 1999, 603, "2022-05-10T18:00:00.000Z"),
                ]),
                page_headers(1),
            ),
            (
                200,
                ratings_json(&[("The Matrix", 1999, 603, 8)]),
                page_headers(1),
            ),
            (200, watchlist_json(&[]), page_headers(1)),
        ]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            summary.distinct_ratings, 1,
            "distinct_ratings must count unique rated films, not diary rows"
        );
        assert_eq!(
            summary.ratings_in_diary, 2,
            "ratings_in_diary counts diary rows with a rating (rewatch-inflated)"
        );
        assert_eq!(summary.diary_rows, 2, "two diary rows for two watches");
        assert!(summary.errored.is_empty());
    }

    #[test]
    fn two_distinct_films_one_rewatched_gives_correct_ratings_counts() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(vec![
            (
                200,
                history_json(&[
                    ("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z"),
                    ("The Matrix", 1999, 603, "2022-05-10T18:00:00.000Z"),
                    ("Inception", 2010, 27205, "2010-07-16T00:00:00.000Z"),
                ]),
                page_headers(1),
            ),
            (
                200,
                ratings_json(&[("The Matrix", 1999, 603, 8), ("Inception", 2010, 27205, 9)]),
                page_headers(1),
            ),
            (200, watchlist_json(&[]), page_headers(1)),
            (200, "[]".to_string(), page_headers(1)),
        ]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            summary.diary_rows, 3,
            "three diary rows (Matrix x2 + Inception)"
        );
        assert_eq!(
            summary.distinct_ratings, 2,
            "distinct_ratings counts unique rated films (Matrix + Inception)"
        );
        assert_eq!(
            summary.ratings_in_diary, 3,
            "ratings_in_diary counts all diary rows that carry a rating (3)"
        );
        assert!(
            summary.ratings_in_diary > summary.distinct_ratings,
            "rewatch scenario: ratings_in_diary ({}) must exceed distinct_ratings ({})",
            summary.ratings_in_diary,
            summary.distinct_ratings
        );
        assert!(summary.errored.is_empty());
    }

    #[test]
    fn errored_is_empty_for_normal_runs() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert!(
            summary.errored.is_empty(),
            "no errored items expected on a successful run"
        );
    }

    // --- FG-17 bucket tests ---

    #[test]
    fn lb_export_already_dated_film_is_skipped() {
        let data_dir = TempDir::new().unwrap();
        // Matrix is in LB diary WITH a watched_date → skipped_existing
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "1999-03-31")]);

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(summary.skipped_existing, 1, "dated film must be skipped");
        assert_eq!(
            summary.diary_rows, 0,
            "no rows emitted for skipped_existing"
        );
        assert_eq!(summary.enriched, 0);
        assert_eq!(summary.skipped_bulk, 0);
        assert_eq!(summary.net_new_bulk, 0);
    }

    #[test]
    fn lb_export_dateless_film_clean_day_is_enriched() {
        let data_dir = TempDir::new().unwrap();
        // Matrix is in LB diary WITHOUT a watched_date (dateless), and watch day is clean.
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "")]);

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(
            summary.enriched, 1,
            "dateless film on clean day must enrich"
        );
        assert_eq!(summary.diary_rows, 1, "enriched film emits a diary row");
        assert_eq!(summary.skipped_existing, 0);
        assert_eq!(summary.skipped_bulk, 0);
        assert_eq!(summary.net_new_bulk, 0);

        // Verify the emitted row has the Trakt date
        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let lines: Vec<&str> = diary.lines().collect();
        assert!(
            lines[1].contains("2024-01-15"),
            "enriched row must carry Trakt date: {}",
            lines[1]
        );
    }

    #[test]
    fn lb_export_dateless_film_bulk_day_is_skipped_bulk() {
        let data_dir = TempDir::new().unwrap();
        // Matrix is dateless on LB; all 10 films are on the same day (bulk cluster).
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "")]);

        // 10 films on the same day triggers bulk threshold.
        let bulk_day = "2023-09-10T00:00:00.000Z";
        let history_entries: Vec<(&str, u32, u64, &str)> = vec![
            ("The Matrix", 1999, 603, bulk_day),
            ("Film B", 2001, 1, bulk_day),
            ("Film C", 2002, 2, bulk_day),
            ("Film D", 2003, 3, bulk_day),
            ("Film E", 2004, 4, bulk_day),
            ("Film F", 2005, 5, bulk_day),
            ("Film G", 2006, 6, bulk_day),
            ("Film H", 2007, 7, bulk_day),
            ("Film I", 2008, 8, bulk_day),
            ("Film J", 2009, 9, bulk_day),
        ];

        let client = MockClient::new(standard_responses(&history_entries, &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(
            summary.skipped_bulk, 1,
            "dateless film on bulk day must be skipped"
        );
        // The other 9 films are net-new on a bulk day → net_new_bulk
        assert_eq!(summary.net_new_bulk, 9);
        assert_eq!(summary.diary_rows, 9, "9 net-new bulk rows emitted");
        assert_eq!(summary.skipped_existing, 0);
        assert_eq!(summary.enriched, 0);
    }

    #[test]
    fn lb_export_net_new_bulk_day_emits_blank_date() {
        let data_dir = TempDir::new().unwrap();
        // No LB export entry for Matrix (net-new), but the day is bulk.
        let lb_dir = TempDir::new().unwrap(); // empty — no diary.csv
                                              // Write empty diary.csv to avoid load errors
        std::fs::write(
            lb_dir.path().join("diary.csv"),
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n",
        )
        .unwrap();

        let bulk_day = "2023-09-10T00:00:00.000Z";
        let mut entries: Vec<(&str, u32, u64, &str)> = vec![("The Matrix", 1999, 603, bulk_day)];
        for i in 1..10usize {
            entries.push(("Other Film", 2000 + i as u32, i as u64, bulk_day));
        }

        let client = MockClient::new(standard_responses(&entries, &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(summary.net_new_bulk, 10, "all 10 films are net-new bulk");
        assert_eq!(summary.diary_rows, 10);

        // Each emitted row must have a blank WatchedDate
        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        for record in rdr.records() {
            let record = record.unwrap();
            assert_eq!(
                &record[3], "",
                "net-new bulk row must have blank WatchedDate, got: {:?}",
                &record[3]
            );
        }
    }

    #[test]
    fn lb_export_net_new_clean_day_emits_with_date() {
        let data_dir = TempDir::new().unwrap();
        // Matrix is not in LB at all, and the day has only 1 film (clean).
        let lb_dir = TempDir::new().unwrap();
        std::fs::write(
            lb_dir.path().join("diary.csv"),
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n",
        )
        .unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(summary.net_new_bulk, 0);
        assert_eq!(summary.diary_rows, 1);

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let lines: Vec<&str> = diary.lines().collect();
        assert!(
            lines[1].contains("2024-01-15"),
            "net-new clean row must have Trakt date: {}",
            lines[1]
        );
    }

    #[test]
    fn title_normalisation_case_insensitive_and_strips_article() {
        let data_dir = TempDir::new().unwrap();
        // LB has "The Matrix" dated; Trakt has "the matrix" (lowercase).
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "1999-03-31")]);

        let client = MockClient::new(standard_responses(
            &[("the matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(
            summary.skipped_existing, 1,
            "'the matrix' (Trakt) must match 'The Matrix' (LB) case-insensitively"
        );
    }

    #[test]
    fn title_normalisation_strips_leading_a_article() {
        let data_dir = TempDir::new().unwrap();
        // LB has "A Quiet Place" dated; Trakt has "A Quiet Place".
        // After normalize: both become "quiet place".
        let lb_dir = make_lb_export_dir(&[("A Quiet Place", 2018, "2018-04-06")]);

        let client = MockClient::new(standard_responses(
            &[("A Quiet Place", 2018, 99999, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(
            summary.skipped_existing, 1,
            "'A Quiet Place' must match (leading 'a ' stripped)"
        );
    }

    #[test]
    fn no_lb_export_new_summary_fields_are_zero() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(summary.enriched, 0);
        assert_eq!(summary.skipped_existing, 0);
        assert_eq!(summary.skipped_bulk, 0);
        assert_eq!(summary.net_new_bulk, 0);
        assert_eq!(summary.diary_rows, 1);
    }

    #[test]
    fn include_ratings_false_suppresses_rating_in_csv() {
        let data_dir = TempDir::new().unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)],
            &[],
        ));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            false, // include_ratings = false
        )
        .unwrap();

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[4], "",
            "Rating column must be empty when include_ratings=false"
        );
    }

    // Cross-run regression: skipped_bulk and skipped_existing films must NOT be recorded
    // in SyncState, so they remain re-classifiable on subsequent runs.
    // --- FG-17 gap tests: boundary, CSV content, "an" article, oracle ---

    #[test]
    fn bulk_boundary_nine_films_is_not_bulk() {
        // 9 films on one day — exactly one below BULK_DATE_THRESHOLD (10) — must NOT trigger bulk.
        let data_dir = TempDir::new().unwrap();
        let lb_dir = TempDir::new().unwrap();
        std::fs::write(
            lb_dir.path().join("diary.csv"),
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n",
        )
        .unwrap();

        let day = "2023-09-10T00:00:00.000Z";
        let history_entries: &[(&str, u32, u64, &str)] = &[
            ("Film A", 2001, 1, day),
            ("Film B", 2002, 2, day),
            ("Film C", 2003, 3, day),
            ("Film D", 2004, 4, day),
            ("Film E", 2005, 5, day),
            ("Film F", 2006, 6, day),
            ("Film G", 2007, 7, day),
            ("Film H", 2008, 8, day),
            ("Film I", 2009, 9, day),
        ];

        let client = MockClient::new(standard_responses(history_entries, &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        assert_eq!(
            summary.net_new_bulk, 0,
            "9 films on one day is below BULK_DATE_THRESHOLD (10); must NOT be bulk"
        );
        assert_eq!(summary.diary_rows, 9, "all 9 net-new films must be emitted");

        // All rows must carry their Trakt date, not a blank
        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        for record in rdr.records() {
            let record = record.unwrap();
            assert_eq!(
                &record[3], "2023-09-10",
                "net-new on 9-film (clean) day must carry Trakt date, not blank"
            );
        }
    }

    #[test]
    fn bulk_boundary_exactly_ten_films_triggers_bulk() {
        // Exactly 10 films on one day — hits BULK_DATE_THRESHOLD; all rows get blank WatchedDate.
        let data_dir = TempDir::new().unwrap();
        let lb_dir = TempDir::new().unwrap();
        std::fs::write(
            lb_dir.path().join("diary.csv"),
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n",
        )
        .unwrap();

        let day = "2023-09-10T00:00:00.000Z";
        let history_entries: &[(&str, u32, u64, &str)] = &[
            ("Film A", 2001, 1, day),
            ("Film B", 2002, 2, day),
            ("Film C", 2003, 3, day),
            ("Film D", 2004, 4, day),
            ("Film E", 2005, 5, day),
            ("Film F", 2006, 6, day),
            ("Film G", 2007, 7, day),
            ("Film H", 2008, 8, day),
            ("Film I", 2009, 9, day),
            ("Film J", 2010, 10, day),
        ];

        let client = MockClient::new(standard_responses(history_entries, &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        assert_eq!(
            summary.net_new_bulk, 10,
            "10 films must hit BULK_DATE_THRESHOLD and be classified net_new_bulk"
        );
        assert_eq!(
            summary.diary_rows, 10,
            "all 10 net-new bulk rows must be emitted"
        );

        // Every row must have a BLANK WatchedDate
        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        for record in rdr.records() {
            let record = record.unwrap();
            assert_eq!(
                &record[3], "",
                "WatchedDate must be blank at exactly the bulk threshold (10 films)"
            );
        }
    }

    #[test]
    fn skipped_existing_film_absent_from_csv() {
        // skipped_existing: dated on LB → no diary row emitted → header-only CSV.
        let data_dir = TempDir::new().unwrap();
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "1999-03-31")]);

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[],
            &[],
        ));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let lines: Vec<&str> = diary.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "skipped_existing film must produce header-only diary CSV; got: {:?}",
            lines
        );
        assert!(
            !diary.contains("The Matrix"),
            "skipped_existing title must be absent from diary CSV"
        );
    }

    #[test]
    fn skipped_bulk_film_absent_from_csv() {
        // A dateless-on-LB film on a bulk day (skipped_bulk) must NOT appear in the diary CSV.
        let data_dir = TempDir::new().unwrap();
        let lb_dir = make_lb_export_dir(&[("The Matrix", 1999, "")]); // dateless on LB

        let bulk_day = "2023-09-10T00:00:00.000Z";
        let history_entries: &[(&str, u32, u64, &str)] = &[
            ("The Matrix", 1999, 603, bulk_day), // skipped_bulk
            ("Film B", 2001, 1, bulk_day),
            ("Film C", 2002, 2, bulk_day),
            ("Film D", 2003, 3, bulk_day),
            ("Film E", 2004, 4, bulk_day),
            ("Film F", 2005, 5, bulk_day),
            ("Film G", 2006, 6, bulk_day),
            ("Film H", 2007, 7, bulk_day),
            ("Film I", 2008, 8, bulk_day),
            ("Film J", 2009, 9, bulk_day),
        ];

        let client = MockClient::new(standard_responses(history_entries, &[], &[]));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        assert!(
            !diary.contains("The Matrix"),
            "skipped_bulk film must NOT appear in diary CSV"
        );
        // The 9 net-new bulk films MUST still be emitted
        assert!(
            diary.contains("Film B"),
            "net_new_bulk films must still be emitted to CSV"
        );
    }

    #[test]
    fn include_ratings_true_with_lb_export_emits_rating() {
        // With include_ratings=true AND a lb_export, the Rating column must be populated
        // for a net-new clean film that has a Trakt rating.
        let data_dir = TempDir::new().unwrap();
        let lb_dir = TempDir::new().unwrap();
        std::fs::write(
            lb_dir.path().join("diary.csv"),
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n",
        )
        .unwrap();

        let client = MockClient::new(standard_responses(
            &[("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z")],
            &[("The Matrix", 1999, 603, 8)], // Trakt 8 → LB 4.0
            &[],
        ));

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true, // include_ratings = true
        )
        .unwrap();

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[4], "4.0",
            "Rating column must be '4.0' (Trakt 8 → LB 4.0) when include_ratings=true with lb_export"
        );
    }

    #[test]
    fn title_normalisation_strips_leading_an_article() {
        // LB has "An American Werewolf in London" (dated); Trakt has the same title.
        // normalize_title strips "an " prefix; both become "american werewolf in london".
        let data_dir = TempDir::new().unwrap();
        let lb_dir = make_lb_export_dir(&[("An American Werewolf in London", 1981, "1981-08-21")]);

        let client = MockClient::new(standard_responses(
            &[(
                "An American Werewolf in London",
                1981,
                55430,
                "2024-01-15T20:30:00.000Z",
            )],
            &[],
            &[],
        ));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        assert_eq!(
            summary.skipped_existing, 1,
            "'An American Werewolf in London' must match (leading 'an ' stripped by normalize_title)"
        );
    }

    #[test]
    fn real_world_oracle_all_buckets_scenario() {
        // Comprehensive oracle: bulk cluster (10 films) + clean-day enrichment + clean-day net-new.
        //   skipped_existing (1): Inception on bulk day, dated on LB
        //   skipped_bulk     (1): The Matrix on bulk day, dateless on LB
        //   net_new_bulk     (8): Film B–I on bulk day, not in LB
        //   enriched         (1): Dunno on clean day, dateless on LB → emits with Trakt date
        //   net_new_clean    (1): The Dark Knight on separate clean day, not in LB
        //   diary_rows = 8 + 1 + 1 = 10
        let data_dir = TempDir::new().unwrap();
        let lb_dir = make_lb_export_dir(&[
            ("Inception", 2010, "2010-07-16"), // dated → skipped_existing
            ("The Matrix", 1999, ""),          // dateless → skipped_bulk (bulk day)
            ("Dunno", 2000, ""),               // dateless → enriched (clean day)
        ]);

        let bulk_day = "2023-09-10T00:00:00.000Z";
        let clean_day_1 = "2024-01-15T20:00:00.000Z"; // Dunno watch
        let clean_day_2 = "2024-01-16T20:00:00.000Z"; // Dark Knight watch

        let history: &[(&str, u32, u64, &str)] = &[
            // Bulk cluster (10 films → bulk threshold triggered)
            ("Inception", 2010, 27205, bulk_day), // skipped_existing
            ("The Matrix", 1999, 603, bulk_day),  // skipped_bulk
            ("Film B", 2001, 1, bulk_day),        // net_new_bulk
            ("Film C", 2002, 2, bulk_day),        // net_new_bulk
            ("Film D", 2003, 3, bulk_day),        // net_new_bulk
            ("Film E", 2004, 4, bulk_day),        // net_new_bulk
            ("Film F", 2005, 5, bulk_day),        // net_new_bulk
            ("Film G", 2006, 6, bulk_day),        // net_new_bulk
            ("Film H", 2007, 7, bulk_day),        // net_new_bulk
            ("Film I", 2008, 8, bulk_day),        // net_new_bulk
            // Clean days
            ("Dunno", 2000, 999, clean_day_1),           // enriched
            ("The Dark Knight", 2008, 155, clean_day_2), // net_new_clean
        ];

        let client = MockClient::new(standard_responses(history, &[], &[]));

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            false,
        )
        .unwrap();

        assert_eq!(
            summary.skipped_existing, 1,
            "Inception must be skipped_existing"
        );
        assert_eq!(summary.skipped_bulk, 1, "Matrix must be skipped_bulk");
        assert_eq!(summary.net_new_bulk, 8, "Film B-I must be net_new_bulk");
        assert_eq!(summary.enriched, 1, "Dunno must be enriched");
        assert_eq!(
            summary.diary_rows, 10,
            "8 net_new_bulk + 1 enriched + 1 net_new_clean = 10 diary rows"
        );

        let diary =
            std::fs::read_to_string(data_dir.path().join("letterboxd-diary-import.csv")).unwrap();

        // Skipped films must be absent from CSV
        assert!(
            !diary.contains("Inception"),
            "skipped_existing must be absent from diary CSV"
        );
        assert!(
            !diary.contains("The Matrix"),
            "skipped_bulk must be absent from diary CSV"
        );

        // Verify date semantics for each emitted bucket
        let mut rdr = csv::Reader::from_reader(diary.as_bytes());
        for record in rdr.records() {
            let record = record.unwrap();
            let title = record.get(0).unwrap_or("");
            let watched_date = record.get(3).unwrap_or("");
            if title == "Dunno" {
                assert_eq!(
                    watched_date, "2024-01-15",
                    "enriched film must carry Trakt date"
                );
            } else if title == "The Dark Knight" {
                assert_eq!(
                    watched_date, "2024-01-16",
                    "net_new_clean film must carry Trakt date"
                );
            } else {
                assert_eq!(
                    watched_date, "",
                    "net_new_bulk film must have blank WatchedDate, got title={title}"
                );
            }
        }
    }

    #[test]
    fn skipped_bulk_and_existing_not_marked_in_sync_state() {
        let data_dir = TempDir::new().unwrap();

        // LB export: Matrix is dated (skipped_existing); Film B is dateless (skipped_bulk).
        let lb_dir = make_lb_export_dir(&[
            ("The Matrix", 1999, "1999-03-31"), // dated -> skipped_existing
            ("Film B", 2001, ""),               // dateless -> skipped_bulk (bulk day)
        ]);

        let bulk_day = "2023-09-10T00:00:00.000Z";
        // 10+ films on bulk_day to trigger the threshold (BULK_DATE_THRESHOLD = 10).
        let history: &[(&str, u32, u64, &str)] = &[
            ("The Matrix", 1999, 603, "2024-01-15T20:30:00.000Z"), // non-bulk day
            ("Film B", 2001, 1, bulk_day),
            ("Film C", 2002, 2, bulk_day),
            ("Film D", 2003, 3, bulk_day),
            ("Film E", 2004, 4, bulk_day),
            ("Film F", 2005, 5, bulk_day),
            ("Film G", 2006, 6, bulk_day),
            ("Film H", 2007, 7, bulk_day),
            ("Film I", 2008, 8, bulk_day),
            ("Film J", 2009, 9, bulk_day),
            ("Film K", 2010, 10, bulk_day),
        ];

        let client1 = MockClient::new(standard_responses(history, &[], &[]));
        let s1 = run(
            &client1,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            Some(lb_dir.path()),
            true,
        )
        .unwrap();

        assert_eq!(s1.skipped_existing, 1, "Matrix must be skipped_existing");
        assert_eq!(s1.skipped_bulk, 1, "Film B must be skipped_bulk");
        // Films C-K (9 net-new on bulk day) are emitted; Matrix and Film B are not.
        assert_eq!(s1.net_new_bulk, 9);
        assert_eq!(s1.diary_rows, 9);

        // Only EMITTED films (C-K) should be recorded in SyncState.
        let state = SyncState::load(data_dir.path());
        let matrix_key = SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "2024-01-15",
        );
        assert!(
            !state.contains(&matrix_key),
            "skipped_existing (Matrix) must not be recorded in SyncState"
        );
        let film_b_key = SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watched,
            ItemRef::Tmdb(1),
            "2023-09-10",
        );
        assert!(
            !state.contains(&film_b_key),
            "skipped_bulk (Film B) must not be recorded in SyncState"
        );

        // Second run without LB export: Matrix and Film B are re-classifiable.
        // Films C-K are in SyncState and are skipped (9 skips); Matrix + Film B emit.
        let client2 = MockClient::new(standard_responses(history, &[], &[]));
        let s2 = run(
            &client2,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
            None,
            true,
        )
        .unwrap();

        assert_eq!(
            s2.skipped, 9,
            "Films C-K (emitted in run 1) must be SyncState-skipped on run 2"
        );
        assert_eq!(
            s2.diary_rows, 2,
            "Matrix and Film B must be re-emittable on second run (not permanently blocked by SyncState)"
        );
    }
}
