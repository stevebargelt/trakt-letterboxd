use std::collections::HashMap;
use std::io;
use std::path::Path;

use crate::{
    letterboxd_import::{write_diary_csv, write_watchlist_csv},
    sync_state::{Direction, ItemRef, ItemType, SyncKey, SyncState},
    trakt_client::TraktHttpClient,
    trakt_notes,
    trakt_read::{
        fetch_ratings, fetch_watched_history, fetch_watchlist, WatchedMovie, WatchlistMovie,
    },
};

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

pub fn run(
    client: &dyn TraktHttpClient,
    data_dir: &Path,
    base_url: &str,
    access_token: &str,
    dry_run: bool,
    force: bool,
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
    let ratings_in_diary = history
        .iter()
        .filter(|e| {
            e.movie
                .tmdb_id
                .map(|id| rating_map.contains_key(&id))
                .unwrap_or(false)
        })
        .count() as u32;
    let reviews_in_diary = history
        .iter()
        .filter(|e| {
            e.movie
                .tmdb_id
                .map(|id| notes.contains_key(&id))
                .unwrap_or(false)
        })
        .count() as u32;

    let diary_rows = history.len() as u32;
    let watchlist_rows = watchlist.len() as u32;

    if !dry_run {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;

        let diary_file = std::fs::File::create(data_dir.join("letterboxd-diary-import.csv"))
            .map_err(|e| format!("failed to create diary CSV: {e}"))?;
        write_diary_csv(io::BufWriter::new(diary_file), &history, &ratings, &notes)?;

        let watchlist_file =
            std::fs::File::create(data_dir.join("letterboxd-watchlist-import.csv"))
                .map_err(|e| format!("failed to create watchlist CSV: {e}"))?;
        write_watchlist_csv(io::BufWriter::new(watchlist_file), &watchlist)?;

        for entry in &history {
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
    // Locks the exact behaviour: run() overwrites the CSV files but produces no data rows when
    // all items are already recorded in sync state.
    #[test]
    fn cross_run_second_export_produces_header_only_csvs() {
        let data_dir = TempDir::new().unwrap();

        // Run 1: export one watched film and one watchlist entry
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
        )
        .unwrap();
        assert_eq!(s1.diary_rows, 1);
        assert_eq!(s1.watchlist_rows, 1);

        // Run 2: identical Trakt data, no --force — both items already in sync state
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
        )
        .unwrap();
        assert_eq!(s2.skipped, 2, "both items must be skipped on second run");
        assert_eq!(s2.diary_rows, 0);
        assert_eq!(s2.watchlist_rows, 0);

        // CSVs are overwritten but contain only the header — no duplicate rows
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

    // GAP 2: Exact diary row content — verifies Trakt rating 8 converts to Letterboxd 4.0,
    // date is truncated to YYYY-MM-DD, and watchlist CSV is a separate file with its own rows only.
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

        // Watchlist is a separate file — must contain only watchlist rows, not diary rows
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

    // GAP 4: Empty account — watchlist CSV must also be header-only (diary already verified in
    // empty_trakt_data_produces_header_only_csvs).
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

    // GAP 5: A watched film with no tmdb_id must appear in the diary CSV with Title+Year
    // (consistent with FG-6 — Letterboxd can match on Title+Year without a tmdb id).
    #[test]
    fn watched_film_without_tmdb_id_appears_in_diary_csv() {
        let data_dir = TempDir::new().unwrap();

        // tmdb:null deserializes as None; film is keyed by TitleYear in sync state
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

    // GAP 6: CSV output files are written at the exact paths that print_to_letterboxd_summary
    // displays to the user — locks the filename contract between run() and the CLI output.
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
        )
        .unwrap();

        assert_eq!(
            summary.reviews_in_diary, 0,
            "no notes available → zero reviews in diary"
        );
    }

    #[test]
    fn distinct_ratings_matches_trakt_ratings_count() {
        // distinct_ratings must equal the number of unique films rated on Trakt,
        // even when the same film appears multiple times in history (rewatches).
        let data_dir = TempDir::new().unwrap();

        // Two history entries for The Matrix (two watches = rewatch), one rating.
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
        // Scenario: 2 distinct rated films, one of which was rewatched.
        //   - The Matrix: watched twice, rated 8
        //   - Inception: watched once, rated 9
        // Expected:
        //   distinct_ratings = 2  (two unique films are rated)
        //   ratings_in_diary = 3  (3 diary rows each carry a rating: 2 Matrix + 1 Inception)
        //   diary_rows       = 3
        // This is the key reconciliation: ratings_in_diary can exceed distinct_ratings.
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
            // notes endpoint — empty (no reviews)
            (200, "[]".to_string(), page_headers(1)),
        ]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            false,
            false,
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
        // The key invariant: ratings_in_diary can legitimately exceed distinct_ratings.
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
        )
        .unwrap();

        assert!(
            summary.errored.is_empty(),
            "no errored items expected on a successful run"
        );
    }
}
