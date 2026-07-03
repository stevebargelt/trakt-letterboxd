use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::constants::BULK_DATE_THRESHOLD;
use crate::{
    letterboxd_export::LetterboxdExport,
    matching::{resolve_films, UnmatchedFilm},
    rating::letterboxd_rating_to_trakt,
    sync_state::{Direction, ItemRef, ItemType, SyncKey, SyncState},
    trakt_client::TraktHttpClient,
    trakt_notes::{self, CreateNoteResult},
    trakt_read,
    trakt_write::{self, HistoryMovie, MovieId, RatingMovie, WatchlistMovie},
};

pub struct ErroredItem {
    pub title: String,
    pub year: u32,
    pub reason: String,
}

pub struct SyncSummary {
    pub watched_added: u32,
    pub watched_on_trakt: u32,
    pub watched_skipped: u32,
    /// watched.csv films whose logged Date fell on a bulk-import day (skipped, not written to Trakt).
    pub watched_bulk_date_skipped: u32,
    pub ratings_added: u32,
    pub ratings_skipped: u32,
    pub watchlist_added: u32,
    pub watchlist_skipped: u32,
    pub unmatched: Vec<UnmatchedFilm>,
    pub errored: Vec<ErroredItem>,
    pub dry_run: bool,
    pub reviews_transferred: u32,
    pub reviews_skipped_over_limit: u32,
    pub reviews_skipped_unmatched: u32,
    /// Films matched via ±1 year tolerance rather than exact year.
    /// Each entry is a human-readable warning for the sync summary.
    pub year_tolerance_warnings: Vec<String>,
}

fn make_ids(tmdb_id: Option<u64>, imdb_id: Option<String>) -> MovieId {
    MovieId { tmdb_id, imdb_id }
}

fn item_ref_for(tmdb_id: Option<u64>, title: &str, year: u32) -> ItemRef {
    match tmdb_id {
        Some(id) => ItemRef::Tmdb(id),
        None => ItemRef::TitleYear(title.to_owned(), year as u16),
    }
}

fn watched_key(tmdb_id: Option<u64>, title: &str, year: u32, date: &str) -> SyncKey {
    SyncKey::new(
        Direction::LetterboxdToTrakt,
        ItemType::Watched,
        item_ref_for(tmdb_id, title, year),
        date,
    )
}

fn rating_key(tmdb_id: Option<u64>, title: &str, year: u32, date: &str) -> SyncKey {
    SyncKey::new(
        Direction::LetterboxdToTrakt,
        ItemType::Rating,
        item_ref_for(tmdb_id, title, year),
        date,
    )
}

fn watchlist_key(tmdb_id: Option<u64>, title: &str, year: u32) -> SyncKey {
    SyncKey::new(
        Direction::LetterboxdToTrakt,
        ItemType::Watchlist,
        item_ref_for(tmdb_id, title, year),
        "",
    )
}

fn review_key(tmdb_id: Option<u64>, title: &str, year: u32, date: &str) -> SyncKey {
    SyncKey::new(
        Direction::LetterboxdToTrakt,
        ItemType::Review,
        item_ref_for(tmdb_id, title, year),
        date,
    )
}

pub fn run(
    client: &dyn TraktHttpClient,
    data_dir: &Path,
    base_url: &str,
    access_token: &str,
    path: &Path,
    dry_run: bool,
    force: bool,
) -> Result<SyncSummary, String> {
    let export = LetterboxdExport::load(path)?;

    let mut state = SyncState::load(data_dir);
    if force {
        state.clear_direction(&Direction::LetterboxdToTrakt);
    }

    // Collect unique (title, year) pairs from all data sources, sorted for determinism.
    let mut unique: HashSet<(String, u32)> = HashSet::new();
    for e in &export.diary {
        unique.insert((e.name.clone(), e.year));
    }
    for e in &export.watched {
        unique.insert((e.name.clone(), e.year));
    }
    for e in &export.ratings {
        unique.insert((e.name.clone(), e.year));
    }
    for e in &export.watchlist {
        unique.insert((e.name.clone(), e.year));
    }
    for e in &export.reviews {
        unique.insert((e.name.clone(), e.year));
    }

    let mut film_list: Vec<(String, u32)> = unique.into_iter().collect();
    film_list.sort();

    let (resolved, unmatched) = resolve_films(client, base_url, access_token, &film_list)?;

    let year_tolerance_warnings: Vec<String> = resolved
        .iter()
        .filter_map(|f| f.year_tolerance_warning.clone())
        .collect();

    // Build lookup: (title_lowercase, year) → (tmdb_id, imdb_id)
    let mut id_map: HashMap<(String, u32), (Option<u64>, Option<String>)> = HashMap::new();
    for film in &resolved {
        id_map.insert(
            (film.title.to_lowercase(), film.year),
            (film.ids.tmdb_id, film.ids.imdb_id.clone()),
        );
    }

    let lookup = |title: &str, year: u32| -> Option<(Option<u64>, Option<String>)> {
        id_map.get(&(title.to_lowercase(), year)).cloned()
    };

    let trakt_history = trakt_read::fetch_watched_history(client, base_url, access_token)?;
    let trakt_watched_ids: HashSet<u64> = trakt_history
        .into_iter()
        .filter_map(|m| m.movie.tmdb_id)
        .collect();

    let mut watched_on_trakt = 0u32;
    let mut watched_skipped = 0u32;
    let mut watched_bulk_date_skipped = 0u32;
    let mut ratings_skipped = 0u32;
    let mut watchlist_skipped = 0u32;
    let mut errored: Vec<ErroredItem> = Vec::new();

    // --- History ---
    // Parallel vecs: movies to write and their key components for state marking.
    let mut history_movies: Vec<HistoryMovie> = Vec::new();
    let mut history_keys: Vec<(Option<u64>, String, u32, String)> = Vec::new();

    // Diary slugs guard against re-adding watched.csv entries already in diary.
    let diary_slugs: HashSet<&str> = export.diary.iter().map(|e| e.slug.as_str()).collect();

    // Precompute bulk days: full pass over watched.csv before the per-film loop.
    // Any calendar day with >= BULK_DATE_THRESHOLD films is a bulk-import day.
    let bulk_days: HashSet<String> = {
        let mut day_counts: HashMap<String, usize> = HashMap::new();
        for entry in &export.watched {
            *day_counts.entry(entry.logged_date.clone()).or_insert(0) += 1;
        }
        day_counts
            .into_iter()
            .filter(|(_, count)| *count >= BULK_DATE_THRESHOLD)
            .map(|(day, _)| day)
            .collect()
    };

    for entry in &export.diary {
        let (tmdb_id, imdb_id) = match lookup(&entry.name, entry.year) {
            Some(ids) => ids,
            None => continue,
        };
        if let Some(id) = tmdb_id {
            if trakt_watched_ids.contains(&id) {
                watched_on_trakt += 1;
                continue;
            }
        }
        let date = if !entry.watched_date.is_empty() {
            entry.watched_date.clone()
        } else {
            entry.logged_date.clone()
        };
        if !force && state.contains(&watched_key(tmdb_id, &entry.name, entry.year, &date)) {
            watched_skipped += 1;
            continue;
        }
        history_movies.push(HistoryMovie {
            ids: make_ids(tmdb_id, imdb_id),
            watched_at: Some(format!("{date}T00:00:00.000Z")),
        });
        history_keys.push((tmdb_id, entry.name.clone(), entry.year, date));
    }

    for entry in &export.watched {
        if diary_slugs.contains(entry.slug.as_str()) {
            continue;
        }
        let (tmdb_id, imdb_id) = match lookup(&entry.name, entry.year) {
            Some(ids) => ids,
            None => continue,
        };
        if let Some(id) = tmdb_id {
            if trakt_watched_ids.contains(&id) {
                watched_on_trakt += 1;
                continue;
            }
        }
        if bulk_days.contains(&entry.logged_date) {
            watched_bulk_date_skipped += 1;
            continue;
        }
        let date = entry.logged_date.clone();
        if !force && state.contains(&watched_key(tmdb_id, &entry.name, entry.year, &date)) {
            watched_skipped += 1;
            continue;
        }
        history_movies.push(HistoryMovie {
            ids: make_ids(tmdb_id, imdb_id),
            watched_at: Some(format!("{date}T00:00:00.000Z")),
        });
        history_keys.push((tmdb_id, entry.name.clone(), entry.year, date));
    }

    // --- Ratings ---
    let mut rating_movies: Vec<RatingMovie> = Vec::new();
    let mut rating_keys: Vec<(Option<u64>, String, u32, String)> = Vec::new();

    for entry in &export.ratings {
        let (tmdb_id, imdb_id) = match lookup(&entry.name, entry.year) {
            Some(ids) => ids,
            None => continue,
        };
        let date = entry.logged_date.clone();
        if !force && state.contains(&rating_key(tmdb_id, &entry.name, entry.year, &date)) {
            ratings_skipped += 1;
            continue;
        }
        rating_movies.push(RatingMovie {
            ids: make_ids(tmdb_id, imdb_id),
            rating: letterboxd_rating_to_trakt(entry.rating),
            rated_at: if !date.is_empty() {
                Some(format!("{date}T00:00:00.000Z"))
            } else {
                None
            },
        });
        rating_keys.push((tmdb_id, entry.name.clone(), entry.year, date));
    }

    // --- Watchlist ---
    let mut watchlist_movies: Vec<WatchlistMovie> = Vec::new();
    let mut watchlist_keys: Vec<(Option<u64>, String, u32)> = Vec::new();

    for entry in &export.watchlist {
        let (tmdb_id, imdb_id) = match lookup(&entry.name, entry.year) {
            Some(ids) => ids,
            None => continue,
        };
        if !force && state.contains(&watchlist_key(tmdb_id, &entry.name, entry.year)) {
            watchlist_skipped += 1;
            continue;
        }
        watchlist_movies.push(WatchlistMovie {
            ids: make_ids(tmdb_id, imdb_id),
        });
        watchlist_keys.push((tmdb_id, entry.name.clone(), entry.year));
    }

    // --- Write or dry run ---
    let mut watched_added = 0u32;
    let mut ratings_added = 0u32;
    let mut watchlist_added = 0u32;

    if dry_run {
        watched_added = history_movies.len() as u32;
        ratings_added = rating_movies.len() as u32;
        watchlist_added = watchlist_movies.len() as u32;
    } else {
        if !history_movies.is_empty() {
            let summary =
                trakt_write::add_to_history(client, base_url, access_token, &history_movies)?;
            watched_added = summary.added;
            for (tmdb_id, title, year, date) in &history_keys {
                state.mark(watched_key(*tmdb_id, title, *year, date));
            }
        }
        if !rating_movies.is_empty() {
            let summary = trakt_write::add_ratings(client, base_url, access_token, &rating_movies)?;
            ratings_added = summary.added;
            for (tmdb_id, title, year, date) in &rating_keys {
                state.mark(rating_key(*tmdb_id, title, *year, date));
            }
        }
        if !watchlist_movies.is_empty() {
            let summary =
                trakt_write::add_to_watchlist(client, base_url, access_token, &watchlist_movies)?;
            watchlist_added = summary.added;
            for (tmdb_id, title, year) in &watchlist_keys {
                state.mark(watchlist_key(*tmdb_id, title, *year));
            }
        }
        state.save(data_dir)?;
    }

    // --- Reviews (best-effort) ---
    let mut reviews_transferred = 0u32;
    let mut reviews_skipped_over_limit = 0u32;
    let mut reviews_skipped_unmatched = 0u32;
    let mut hit_limit = false;

    for review_entry in &export.reviews {
        if review_entry.review.is_empty() {
            continue;
        }

        let (tmdb_id, imdb_id) = match lookup(&review_entry.name, review_entry.year) {
            Some(ids) => ids,
            None => {
                reviews_skipped_unmatched += 1;
                continue;
            }
        };

        let date = if !review_entry.watched_date.is_empty() {
            review_entry.watched_date.clone()
        } else {
            review_entry.logged_date.clone()
        };
        let key = review_key(tmdb_id, &review_entry.name, review_entry.year, &date);

        if !force && state.contains(&key) {
            continue;
        }

        if hit_limit {
            reviews_skipped_over_limit += 1;
            continue;
        }

        if dry_run {
            reviews_transferred += 1;
        } else {
            match trakt_notes::create_note(
                client,
                base_url,
                access_token,
                &review_entry.review,
                tmdb_id,
                imdb_id,
            ) {
                Ok(CreateNoteResult::Created) => {
                    state.mark(key);
                    reviews_transferred += 1;
                }
                Ok(CreateNoteResult::OverLimit) => {
                    hit_limit = true;
                    reviews_skipped_over_limit += 1;
                }
                Err(e) => {
                    eprintln!(
                        "warning: failed to create note for '{}': {e}",
                        review_entry.name
                    );
                    errored.push(ErroredItem {
                        title: review_entry.name.clone(),
                        year: review_entry.year,
                        reason: format!("note creation failed: {e}"),
                    });
                }
            }
        }
    }

    // Persist any new review state marks (best-effort).
    if !dry_run && reviews_transferred > 0 {
        let _ = state.save(data_dir);
    }

    Ok(SyncSummary {
        watched_added,
        watched_on_trakt,
        watched_skipped,
        watched_bulk_date_skipped,
        ratings_added,
        ratings_skipped,
        watchlist_added,
        watchlist_skipped,
        unmatched,
        errored,
        dry_run,
        reviews_transferred,
        reviews_skipped_over_limit,
        reviews_skipped_unmatched,
        year_tolerance_warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trakt_client::{HttpResponse, TraktHttpClient};
    use std::collections::{HashMap, VecDeque};
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct MockClient {
        get_responses: Mutex<VecDeque<(u16, String)>>,
        post_responses: Mutex<VecDeque<(u16, String)>>,
        post_calls: Mutex<Vec<(String, String)>>,
    }

    impl MockClient {
        fn new(get: Vec<(u16, String)>, post: Vec<(u16, String)>) -> Self {
            MockClient {
                get_responses: Mutex::new(get.into()),
                post_responses: Mutex::new(post.into()),
                post_calls: Mutex::new(Vec::new()),
            }
        }

        fn post_count(&self) -> usize {
            self.post_calls.lock().unwrap().len()
        }

        fn post_urls(&self) -> Vec<String> {
            self.post_calls
                .lock()
                .unwrap()
                .iter()
                .map(|(u, _)| u.clone())
                .collect()
        }

        fn post_bodies(&self) -> Vec<String> {
            self.post_calls
                .lock()
                .unwrap()
                .iter()
                .map(|(_, b)| b.clone())
                .collect()
        }
    }

    impl TraktHttpClient for MockClient {
        fn post_json(&self, _url: &str, _body: &str) -> Result<HttpResponse, String> {
            unreachable!("sync_from_letterboxd tests do not call post_json")
        }

        fn post_json_auth(
            &self,
            url: &str,
            body: &str,
            _token: &str,
        ) -> Result<HttpResponse, String> {
            self.post_calls
                .lock()
                .unwrap()
                .push((url.to_string(), body.to_string()));
            let (status, resp) = self
                .post_responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| {
                    (
                        200,
                        r#"{"added":{"movies":0},"not_found":{"movies":[]}}"#.to_string(),
                    )
                });
            Ok(HttpResponse {
                status,
                body: resp,
                headers: HashMap::new(),
            })
        }

        fn get(&self, _url: &str, _token: &str) -> Result<HttpResponse, String> {
            let (status, body) = self
                .get_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("no GET response queued");
            Ok(HttpResponse {
                status,
                body,
                headers: HashMap::new(),
            })
        }
    }

    fn write_csv(dir: &TempDir, name: &str, content: &str) {
        let mut f = std::fs::File::create(dir.path().join(name)).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    fn match_json(title: &str, year: u32, trakt: u64, tmdb: u64) -> String {
        format!(
            r#"[{{"type":"movie","score":1000.0,"movie":{{"title":"{title}","year":{year},"ids":{{"trakt":{trakt},"slug":"s","imdb":"tt1","tmdb":{tmdb}}}}}}}]"#
        )
    }

    fn ok_resp(added: u32) -> String {
        format!(r#"{{"added":{{"movies":{added}}},"not_found":{{"movies":[]}}}}"#)
    }

    fn empty_history() -> (u16, String) {
        (200, "[]".to_string())
    }

    fn trakt_history_json(tmdb_ids: &[u64]) -> (u16, String) {
        let entries: Vec<String> = tmdb_ids
            .iter()
            .map(|id| {
                format!(
                    r#"{{"watched_at":"2024-01-01T00:00:00.000Z","movie":{{"title":"Film","year":2000,"ids":{{"trakt":1,"slug":"s","imdb":"tt1","tmdb":{id}}}}}}}"#
                )
            })
            .collect();
        (200, format!("[{}]", entries.join(",")))
    }

    const DIARY_CSV: &str = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
        2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5,No,,1999-03-31\n";

    const RATINGS_CSV: &str = "Date,Name,Year,Letterboxd URI,Rating\n\
        2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5\n";

    const WATCHLIST_CSV: &str = "Date,Name,Year,Letterboxd URI\n\
        2024-01-15,Dune,2021,https://letterboxd.com/film/dune-2021/\n";

    #[test]
    fn dry_run_makes_no_writes_and_reports_correct_counts() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "ratings.csv", RATINGS_CSV);

        // 1 unique film (diary + ratings deduplicate to The Matrix).
        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![], // no POSTs
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            true,  // dry_run
            false, // force
        )
        .unwrap();

        assert_eq!(client.post_count(), 0, "dry run must make no writes");
        assert!(summary.dry_run);
        assert_eq!(summary.watched_added, 1, "1 diary entry would be added");
        assert_eq!(summary.ratings_added, 1, "1 rating entry would be added");
        assert_eq!(summary.watchlist_added, 0);
        assert_eq!(summary.watched_skipped, 0);
        assert_eq!(summary.ratings_skipped, 0);
        assert_eq!(summary.watchlist_skipped, 0);
        assert!(summary.unmatched.is_empty());
        assert!(summary.errored.is_empty());
    }

    #[test]
    fn dry_run_does_not_write_state_file() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![],
        );

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            true,
            false,
        )
        .unwrap();

        assert!(
            !data_dir.path().join("sync_state.json").exists(),
            "dry run must not write sync state"
        );
    }

    #[test]
    fn real_sync_calls_write_endpoints_with_expected_payloads() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "ratings.csv", RATINGS_CSV);
        write_csv(&export_dir, "watchlist.csv", WATCHLIST_CSV);

        // Sorted alphabetically: Dune (2021) then The Matrix (1999).
        let client = MockClient::new(
            vec![
                (200, match_json("Dune", 2021, 999, 438631)),
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)), // add_to_history
                (200, ok_resp(1)), // add_ratings
                (200, ok_resp(1)), // add_to_watchlist
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert!(!summary.dry_run);
        assert_eq!(summary.watched_added, 1);
        assert_eq!(summary.ratings_added, 1);
        assert_eq!(summary.watchlist_added, 1);
        assert_eq!(summary.watched_skipped, 0);
        assert_eq!(summary.ratings_skipped, 0);
        assert_eq!(summary.watchlist_skipped, 0);
        assert!(summary.unmatched.is_empty());
        assert!(summary.errored.is_empty());

        let urls = client.post_urls();
        assert!(
            urls.iter().any(|u| u.contains("/sync/history")),
            "must POST to history endpoint"
        );
        assert!(
            urls.iter().any(|u| u.contains("/sync/ratings")),
            "must POST to ratings endpoint"
        );
        assert!(
            urls.iter().any(|u| u.contains("/sync/watchlist")),
            "must POST to watchlist endpoint"
        );

        let bodies = client.post_bodies();
        // history body must contain watched_at for 1999-03-31
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("1999-03-31T00:00:00.000Z")),
            "watched_at must be derived from watched_date"
        );
        // ratings body must contain tmdb:603 and rating:9 (4.5 * 2)
        assert!(
            bodies.iter().any(|b| b.contains("\"rating\":9")),
            "4.5 Letterboxd rating must map to 9 on Trakt"
        );
    }

    #[test]
    fn already_synced_items_are_skipped() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        // Pre-mark The Matrix as already synced to history.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "1999-03-31",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![], // no writes expected
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "already-synced item must not trigger a write"
        );
        assert_eq!(summary.watched_skipped, 1);
        assert_eq!(summary.watched_added, 0);
    }

    #[test]
    fn force_flag_resyncs_already_synced_items() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        // Pre-mark as synced.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "1999-03-31",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            true, // force
        )
        .unwrap();

        assert_eq!(client.post_count(), 1, "--force must re-sync the item");
        assert_eq!(summary.watched_skipped, 0);
        assert_eq!(summary.watched_added, 1);
    }

    #[test]
    fn unmatched_films_are_collected_and_no_writes_occur() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        // No match for The Matrix in pass 1 or either adjacent-year pass.
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: years=1999 → no match
                (200, "[]".to_string()), // pass 2: years=1998 → no match
                (200, "[]".to_string()), // pass 2: years=2000 → no match
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(client.post_count(), 0);
        assert_eq!(summary.unmatched.len(), 1);
        assert_eq!(summary.unmatched[0].title, "The Matrix");
        assert_eq!(summary.unmatched[0].year, 1999);
        assert_eq!(summary.watched_added, 0);
    }

    #[test]
    fn state_is_persisted_after_real_sync() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        // The state file must now contain the synced key.
        let state = SyncState::load(data_dir.path());
        assert!(state.contains(&SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "1999-03-31",
        )));
    }

    #[test]
    fn watched_entries_not_in_diary_are_synced_to_history() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // watched.csv entry for a film NOT in diary.csv
        let watched_csv = "Date,Name,Year,Letterboxd URI\n\
            2023-05-01,Inception,2010,https://letterboxd.com/film/inception/\n";
        write_csv(&export_dir, "watched.csv", watched_csv);

        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.watched_added, 1);
        assert!(
            client
                .post_urls()
                .iter()
                .any(|u| u.contains("/sync/history")),
            "watched-only entry must be added to history"
        );
    }

    #[test]
    fn watchlist_skipped_if_already_synced() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "watchlist.csv", WATCHLIST_CSV);

        // Pre-mark Dune as already in watchlist.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watchlist,
            ItemRef::Tmdb(438631),
            "",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(
            vec![
                (200, match_json("Dune", 2021, 999, 438631)),
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(client.post_count(), 0);
        assert_eq!(summary.watchlist_skipped, 1);
        assert_eq!(summary.watchlist_added, 0);
    }

    // ── Gap coverage (FG-9 verify) ────────────────────────────────────────────

    #[test]
    fn title_format_mismatch_lands_in_unmatched() {
        // Letterboxd has "Amelie" (no accent); Trakt returns "Amélie" (accented).
        // The matching logic requires case-insensitive exact equality:
        //   "amelie" != "amélie"  →  search returns None  →  unmatched.
        // No write must occur and the film must appear in unmatched (not silently dropped).
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-06-01,Amelie,2001,https://letterboxd.com/film/amelie/,5.0,No,,2001-04-25\n",
        );

        // Trakt returns the accented "Amélie" — does NOT match the unaccented "Amelie".
        // Adjacent-year passes also return a title mismatch, so Amelie stays unmatched.
        let trakt_resp = r#"[{"type":"movie","score":1000.0,"movie":{"title":"Amélie","year":2001,"ids":{"trakt":123,"slug":"amelie","imdb":"tt0211915","tmdb":194}}}]"#;
        let client = MockClient::new(
            vec![
                (200, trakt_resp.to_string()), // pass 1: years=2001 → Amélie (title mismatch)
                (200, "[]".to_string()),       // pass 2: years=2000 → no match
                (200, "[]".to_string()),       // pass 2: years=2002 → no match
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "title-format mismatch must not trigger any write"
        );
        assert_eq!(
            summary.unmatched.len(),
            1,
            "Amelie must land in unmatched, not silently dropped"
        );
        assert_eq!(summary.unmatched[0].title, "Amelie");
        assert_eq!(summary.unmatched[0].year, 2001);
        assert_eq!(summary.watched_added, 0);
    }

    #[test]
    fn partial_sync_mixed_batch() {
        // Three diary films simultaneously:
        //   "Ghost Film" 2050  →  unmatched (search returns [])
        //   "Inception" 2010   →  already-synced in state → skipped
        //   "The Matrix" 1999  →  new match → added
        //
        // Asserts added/skipped/unmatched counts are all correct at once
        // and only The Matrix triggers a write.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // Films are sorted alphabetically before resolve: Ghost Film < Inception < The Matrix.
        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-10,Ghost Film,2050,https://letterboxd.com/film/ghost-film/,,,,\n\
            2024-01-11,Inception,2010,https://letterboxd.com/film/inception/,,,,2010-07-16\n\
            2024-01-12,The Matrix,1999,https://letterboxd.com/film/the-matrix/,,,,1999-03-31\n",
        );

        // Pre-mark Inception (tmdb:27205, 2010-07-16) as already synced.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(27205),
            "2010-07-16",
        ));
        state.save(data_dir.path()).unwrap();

        // GET responses: pass 1 for all three films, then pass 2 adjacent-year for Ghost Film.
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: Ghost Film → no match
                (200, match_json("Inception", 2010, 123, 27205)), // pass 1: Inception → match
                (200, match_json("The Matrix", 1999, 481, 603)), // pass 1: The Matrix → match
                (200, "[]".to_string()), // pass 2: Ghost Film year-1 (2049)
                (200, "[]".to_string()), // pass 2: Ghost Film year+1 (2051)
                empty_history(),
            ],
            vec![(201, ok_resp(1))], // one write: add_to_history for The Matrix
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_skipped, 1,
            "Inception (already-synced) must be skipped"
        );
        assert_eq!(summary.watched_added, 1, "The Matrix must be added");
        assert_eq!(
            summary.unmatched.len(),
            1,
            "Ghost Film must appear in unmatched"
        );
        assert_eq!(summary.unmatched[0].title, "Ghost Film");
        assert!(summary.errored.is_empty(), "no errors expected");
        assert_eq!(client.post_count(), 1, "exactly one write endpoint called");
    }

    #[test]
    fn empty_export_completes_with_zero_counts() {
        // No CSV files at all — run must complete without error or panic,
        // with all counters at zero and no HTTP calls made.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // No films to resolve, but fetch_watched_history still makes one GET.
        let client = MockClient::new(vec![empty_history()], vec![]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(client.post_count(), 0);
        assert_eq!(summary.watched_added, 0);
        assert_eq!(summary.ratings_added, 0);
        assert_eq!(summary.watchlist_added, 0);
        assert_eq!(summary.watched_skipped, 0);
        assert_eq!(summary.ratings_skipped, 0);
        assert_eq!(summary.watchlist_skipped, 0);
        assert!(summary.unmatched.is_empty());
        assert!(summary.errored.is_empty());
    }

    #[test]
    fn dry_run_with_already_synced_excludes_synced_from_counts() {
        // In dry-run mode, already-synced items must still be excluded from
        // the would-add count.  The skipped counter increments as normal;
        // watched_added must reflect only genuinely new items (zero here).
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        // Pre-mark The Matrix (tmdb:603, 1999-03-31) as already synced.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(603),
            "1999-03-31",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            true,  // dry_run
            false, // no force
        )
        .unwrap();

        assert_eq!(client.post_count(), 0, "dry run must not write");
        assert!(summary.dry_run);
        assert_eq!(
            summary.watched_added, 0,
            "dry-run would-add must exclude already-synced items"
        );
        assert_eq!(
            summary.watched_skipped, 1,
            "already-synced item is counted as skipped even in dry-run"
        );
        assert!(summary.errored.is_empty());
    }

    #[test]
    fn idempotency_second_run_writes_nothing() {
        // Run a full sync (mocked writes), then run again with the same data
        // and no --force.  The second run must write nothing — the state store
        // prevents duplicates across runs.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        // First run: real sync — writes The Matrix to history, saves state.
        let client1 = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );
        let summary1 = run(
            &client1,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();
        assert_eq!(summary1.watched_added, 1, "first run must add the film");
        assert_eq!(
            client1.post_count(),
            1,
            "first run must make exactly one write"
        );

        // Second run: same export, same data dir — state already marks The Matrix synced.
        let client2 = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![], // no POSTs expected
        );
        let summary2 = run(
            &client2,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(client2.post_count(), 0, "second run must make no writes");
        assert_eq!(
            summary2.watched_skipped, 1,
            "second run must skip the already-synced film"
        );
        assert_eq!(summary2.watched_added, 0);
    }

    const REVIEW_CSV: &str = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review\n\
        2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5,No,,1999-03-31,\"Best film ever\"\n";

    #[test]
    fn review_attached_as_note_and_reported_transferred() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)),                // POST /sync/history
                (201, r#"{"id":1}"#.to_string()), // POST /notes
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.reviews_transferred, 1,
            "review must be reported as transferred"
        );
        assert_eq!(summary.reviews_skipped_over_limit, 0);
        assert_eq!(summary.reviews_skipped_unmatched, 0);

        let note_calls: Vec<String> = client
            .post_urls()
            .into_iter()
            .filter(|u| u.contains("/notes"))
            .collect();
        assert_eq!(
            note_calls.len(),
            1,
            "must POST to /notes endpoint exactly once"
        );
    }

    #[test]
    fn review_over_limit_reported_skipped() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)),                                  // POST /sync/history
                (422, r#"{"error":"limit exceeded"}"#.to_string()), // POST /notes - over limit
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.reviews_transferred, 0);
        assert_eq!(
            summary.reviews_skipped_over_limit, 1,
            "over-limit must be reported skipped"
        );
        assert_eq!(summary.reviews_skipped_unmatched, 0);
    }

    #[test]
    fn review_unmatched_film_reported_skipped() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let unmatched_review = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review\n\
            2024-01-15,Unknown Film,2099,https://letterboxd.com/film/unknown-film/,,,,,Great film\n";
        write_csv(&export_dir, "reviews.csv", unmatched_review);

        // No match for Unknown Film in any pass.
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: years=2099 → no match
                (200, "[]".to_string()), // pass 2: years=2098 → no match
                (200, "[]".to_string()), // pass 2: years=2100 → no match
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.reviews_transferred, 0);
        assert_eq!(
            summary.reviews_skipped_unmatched, 1,
            "unmatched review must be reported skipped"
        );
        assert_eq!(summary.reviews_skipped_over_limit, 0);
        assert_eq!(
            client.post_count(),
            0,
            "no write should happen for unmatched review"
        );
    }

    #[test]
    fn review_dry_run_does_not_post_note() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![], // no POSTs expected in dry run
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            true, // dry_run
            false,
        )
        .unwrap();

        assert_eq!(client.post_count(), 0, "dry run must not POST anything");
        assert!(summary.dry_run);
        assert_eq!(
            summary.reviews_transferred, 1,
            "dry run must report would-transfer count"
        );
    }

    #[test]
    fn review_already_synced_is_silently_skipped() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        // Pre-mark the review as already synced.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Review,
            ItemRef::Tmdb(603),
            "1999-03-31",
        ));
        state.save(data_dir.path()).unwrap();

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))], // only history write, no notes write
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.reviews_transferred, 0,
            "already-synced review must be silently skipped"
        );
        assert_eq!(summary.reviews_skipped_over_limit, 0);
        assert_eq!(summary.reviews_skipped_unmatched, 0);

        let note_calls: Vec<String> = client
            .post_urls()
            .into_iter()
            .filter(|u| u.contains("/notes"))
            .collect();
        assert_eq!(
            note_calls.len(),
            0,
            "must not re-post an already-synced review"
        );
    }

    #[test]
    fn over_limit_review_does_not_abort_history_sync() {
        // History write must complete even when the note POST hits the free-tier limit.
        // The review section runs AFTER history/ratings/watchlist, so an over-limit
        // response must never roll back or abort the already-persisted history.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        // One film: The Matrix. History POST succeeds; note POST hits limit.
        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)),                                  // POST /sync/history
                (422, r#"{"error":"limit exceeded"}"#.to_string()), // POST /notes — over limit
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap(); // must NOT abort

        assert_eq!(
            summary.watched_added, 1,
            "history sync must complete despite over-limit note rejection"
        );
        assert_eq!(
            summary.reviews_skipped_over_limit, 1,
            "over-limit review must be counted in skipped_over_limit"
        );
        assert_eq!(summary.reviews_transferred, 0);
        assert!(!summary.dry_run);

        // State file must exist — history was persisted before reviews were attempted.
        assert!(
            data_dir.path().join("sync_state.json").exists(),
            "sync state must be saved even when a review hits the limit"
        );
    }

    #[test]
    fn second_review_skipped_after_over_limit() {
        // If the first note hits the limit, subsequent reviews in the same run
        // must be counted as skipped (not attempted).
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let multi_review = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review\n\
            2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5,No,,1999-03-31,Review one\n\
            2024-01-16,Inception,2010,https://letterboxd.com/film/inception/,4.5,No,,2010-07-16,Review two\n";
        write_csv(&export_dir, "reviews.csv", multi_review);

        // Films sorted: Inception < The Matrix
        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (422, r#"{"error":"limit"}"#.to_string()), // first note hits limit
                                                           // no second note call expected
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.reviews_transferred, 0);
        assert_eq!(
            summary.reviews_skipped_over_limit, 2,
            "both reviews must be counted as skipped after limit is hit"
        );

        let note_calls: Vec<String> = client
            .post_urls()
            .into_iter()
            .filter(|u| u.contains("/notes"))
            .collect();
        assert_eq!(
            note_calls.len(),
            1,
            "only one /notes POST attempted before limit"
        );
    }

    #[test]
    fn review_api_error_goes_to_errored_not_over_limit() {
        // A 500 from the notes endpoint is a real failure, not an over-limit condition.
        // It must appear in errored[], not reviews_skipped_over_limit.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)),                                // POST /sync/history
                (500, r#"{"error":"server error"}"#.to_string()), // POST /notes — real error
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.reviews_transferred, 0);
        assert_eq!(
            summary.reviews_skipped_over_limit, 0,
            "a 500 is not an over-limit skip"
        );
        assert_eq!(
            summary.errored.len(),
            1,
            "a 500 from /notes must appear in errored"
        );
        assert_eq!(summary.errored[0].title, "The Matrix");
        assert_eq!(summary.errored[0].year, 1999);
        assert!(
            summary.errored[0].reason.contains("500"),
            "reason must mention the HTTP status: {}",
            summary.errored[0].reason
        );
    }

    #[test]
    fn write_error_goes_to_errored_not_unmatched() {
        // A film that RESOLVES successfully but whose note POST returns HTTP 500
        // must land in errored[], NOT in unmatched[].
        // This locks the separation: resolve failure -> unmatched; write failure -> errored.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);
        write_csv(&export_dir, "reviews.csv", REVIEW_CSV);

        // The Matrix resolves (GET returns a match), history POST succeeds,
        // but note POST returns 500 — a genuine write error.
        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                empty_history(),
            ],
            vec![
                (201, ok_resp(1)),                                  // POST /sync/history
                (500, r#"{"error":"internal error"}"#.to_string()), // POST /notes — write error
            ],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        // Write failure on a resolved film -> errored, not unmatched.
        assert_eq!(
            summary.errored.len(),
            1,
            "a 500 write error must produce exactly one errored item"
        );
        assert_eq!(summary.errored[0].title, "The Matrix");
        assert!(
            summary.unmatched.is_empty(),
            "a resolved film that errors on write must NOT appear in unmatched"
        );
    }

    #[test]
    fn summary_fields_include_title_year_reason_for_unmatched() {
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: years=1999 → no match
                (200, "[]".to_string()), // pass 2: years=1998 → no match
                (200, "[]".to_string()), // pass 2: years=2000 → no match
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.unmatched.len(), 1);
        assert_eq!(summary.unmatched[0].title, "The Matrix");
        assert_eq!(summary.unmatched[0].year, 1999);
        assert!(
            !summary.unmatched[0].reason.is_empty(),
            "unmatched must include a reason"
        );
        assert!(summary.errored.is_empty(), "no errors for unmatched film");
    }

    // ── FG-16: skip films already in Trakt history ────────────────────────────

    #[test]
    fn diary_film_already_on_trakt_is_skipped_not_duplicated() {
        // The Matrix is already in the user's Trakt history (tmdb 603).
        // It must NOT get a second play entry; watched_on_trakt must be 1.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                trakt_history_json(&[603]),
            ],
            vec![], // no POSTs — already on Trakt
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "film already on Trakt must not trigger a write"
        );
        assert_eq!(
            summary.watched_on_trakt, 1,
            "already-on-Trakt film must be counted in watched_on_trakt"
        );
        assert_eq!(summary.watched_added, 0);
        assert_eq!(
            summary.watched_skipped, 0,
            "already-on-Trakt is a distinct bucket from already-synced"
        );
    }

    #[test]
    fn new_film_not_on_trakt_is_added_normally() {
        // Inception is NOT in Trakt history; The Matrix IS.
        // Only Inception should be added.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-11,Inception,2010,https://letterboxd.com/film/inception/,,,,2010-07-16\n\
            2024-01-12,The Matrix,1999,https://letterboxd.com/film/the-matrix/,,,,1999-03-31\n",
        );

        // Films sorted: Inception < The Matrix.
        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                (200, match_json("The Matrix", 1999, 481, 603)),
                trakt_history_json(&[603]), // The Matrix already on Trakt
            ],
            vec![(201, ok_resp(1))], // one write: Inception
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.watched_added, 1, "Inception must be added");
        assert_eq!(
            summary.watched_on_trakt, 1,
            "The Matrix must be counted as already-on-Trakt"
        );
        assert_eq!(summary.watched_skipped, 0);
        assert_eq!(client.post_count(), 1);

        // The POST body must contain Inception's watched date, not The Matrix's.
        let bodies = client.post_bodies();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("2010-07-16T00:00:00.000Z")),
            "watched_at must use Inception's watched date"
        );
        assert!(
            !bodies
                .iter()
                .any(|b| b.contains("1999-03-31T00:00:00.000Z")),
            "The Matrix must not appear in any POST body"
        );
    }

    #[test]
    fn dateless_watched_csv_film_already_on_trakt_is_skipped() {
        // watched.csv entries have only a logged Date (no real watch date).
        // If the film is already in Trakt history it must still be skipped —
        // this is the primary duplication scenario described in FG-16.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let watched_csv = "Date,Name,Year,Letterboxd URI\n\
            2024-01-15,Inception,2010,https://letterboxd.com/film/inception/\n";
        write_csv(&export_dir, "watched.csv", watched_csv);

        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                trakt_history_json(&[27205]), // already on Trakt
            ],
            vec![], // no POSTs
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "dateless watched.csv film already on Trakt must not be re-added"
        );
        assert_eq!(summary.watched_on_trakt, 1);
        assert_eq!(summary.watched_added, 0);
        assert_eq!(summary.watched_skipped, 0);
    }

    #[test]
    fn dry_run_already_on_trakt_is_counted_not_posted() {
        // In dry-run mode, a film already in Trakt history must still be
        // counted in watched_on_trakt — the skip decision happens before the
        // dry-run write gate, so dry-run must reflect it accurately.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV);

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)),
                trakt_history_json(&[603]), // already on Trakt
            ],
            vec![], // no POSTs — dry run AND already on Trakt
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            true, // dry_run
            false,
        )
        .unwrap();

        assert_eq!(client.post_count(), 0, "dry run must not POST anything");
        assert!(summary.dry_run);
        assert_eq!(
            summary.watched_on_trakt, 1,
            "dry-run must still count the already-on-Trakt film"
        );
        assert_eq!(
            summary.watched_added, 0,
            "already-on-Trakt film must not appear in would-add count"
        );
        assert_eq!(
            summary.watched_skipped, 0,
            "distinct from already-synced bucket"
        );
    }

    #[test]
    fn three_bucket_counts_are_distinct_and_correct_in_single_run() {
        // Three films hit each bucket simultaneously:
        //   Inception (tmdb 27205) → already on Trakt       → watched_on_trakt += 1
        //   Parasite  (tmdb 496243)→ already synced in state → watched_skipped  += 1
        //   The Matrix(tmdb 603)   → genuinely new           → watched_added     += 1
        //
        // Asserts the counts are all distinct and only The Matrix triggers a POST.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-11,Inception,2010,https://letterboxd.com/film/inception/,,,,2010-07-16\n\
            2024-01-12,Parasite,2019,https://letterboxd.com/film/parasite/,,,,2019-05-30\n\
            2024-01-13,The Matrix,1999,https://letterboxd.com/film/the-matrix/,,,,1999-03-31\n",
        );

        // Pre-mark Parasite as already synced.
        let mut state = SyncState::load(data_dir.path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(496243),
            "2019-05-30",
        ));
        state.save(data_dir.path()).unwrap();

        // Films sorted: Inception < Parasite < The Matrix.
        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                (200, match_json("Parasite", 2019, 456, 496243)),
                (200, match_json("The Matrix", 1999, 481, 603)),
                trakt_history_json(&[27205]), // Inception already on Trakt
            ],
            vec![(201, ok_resp(1))], // only The Matrix gets added
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(summary.watched_added, 1, "The Matrix must be added");
        assert_eq!(
            summary.watched_on_trakt, 1,
            "Inception must be counted as already-on-Trakt"
        );
        assert_eq!(
            summary.watched_skipped, 1,
            "Parasite must be counted as already-synced"
        );
        assert_eq!(
            client.post_count(),
            1,
            "only one POST — The Matrix to /sync/history"
        );
        // The POST body must contain The Matrix's watch date, not Inception's or Parasite's.
        let bodies = client.post_bodies();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("1999-03-31T00:00:00.000Z")),
            "POST body must contain The Matrix watch date"
        );
        assert!(
            !bodies
                .iter()
                .any(|b| b.contains("2010-07-16T00:00:00.000Z")),
            "Inception (already on Trakt) must not appear in any POST body"
        );
        assert!(
            !bodies
                .iter()
                .any(|b| b.contains("2019-05-30T00:00:00.000Z")),
            "Parasite (already synced) must not appear in any POST body"
        );
    }

    // ── FG-15: year-tolerance warning propagation through run() ──────────────

    #[test]
    fn year_tolerance_match_populates_year_tolerance_warnings_in_summary() {
        // Diary has "Coco" listed as year 2018. Trakt has it under 2017 (festival year).
        // Pass 1 (exact years=2018) → no match.
        // Pass 2 (years=2017) → match → run() must populate year_tolerance_warnings.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-15,Coco,2018,https://letterboxd.com/film/coco/,,,,2018-01-15\n",
        );

        let coco_trakt_json = match_json("Coco", 2017, 100, 354859);
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: years=2018 → no match
                (200, coco_trakt_json),  // pass 2: years=2017 → match
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.year_tolerance_warnings.len(),
            1,
            "year-tolerance match must populate year_tolerance_warnings; got {:?}",
            summary.year_tolerance_warnings
        );
        let warn = &summary.year_tolerance_warnings[0];
        assert!(
            warn.contains("Coco"),
            "warning must mention the film title; got: {warn}"
        );
        assert!(
            warn.contains("2017") && warn.contains("2018"),
            "warning must mention both LB year (2018) and Trakt year (2017); got: {warn}"
        );
        assert!(
            summary.unmatched.is_empty(),
            "near-year matched film must not appear in unmatched"
        );
        assert_eq!(
            summary.watched_added, 1,
            "matched film must be added to history"
        );
    }

    #[test]
    fn year_tolerance_plus_one_match_populates_year_tolerance_warnings_in_summary() {
        // Diary has "Parasite" listed as year 2018. Trakt has it under 2019.
        // Pass 1 (years=2018) → no match.
        // Pass 2 (years=2017) → no match.
        // Pass 2 (years=2019) → match.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-15,Parasite,2018,https://letterboxd.com/film/parasite/,,,,2018-05-21\n",
        );

        let parasite_trakt_json = match_json("Parasite", 2019, 456, 496243);
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()),    // pass 1: years=2018 → no match
                (200, "[]".to_string()),    // pass 2: years=2017 → no match
                (200, parasite_trakt_json), // pass 2: years=2019 → match
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.year_tolerance_warnings.len(),
            1,
            "year+1 tolerance match must populate year_tolerance_warnings"
        );
        let warn = &summary.year_tolerance_warnings[0];
        assert!(
            warn.contains("2019") && warn.contains("2018"),
            "warning must mention both LB year (2018) and Trakt year (2019); got: {warn}"
        );
        assert!(summary.unmatched.is_empty());
        assert_eq!(summary.watched_added, 1);
    }

    #[test]
    fn exact_year_match_leaves_year_tolerance_warnings_empty() {
        // An exact title+year match must never populate year_tolerance_warnings.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(&export_dir, "diary.csv", DIARY_CSV); // The Matrix 1999

        let client = MockClient::new(
            vec![
                (200, match_json("The Matrix", 1999, 481, 603)), // pass 1: exact match
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert!(
            summary.year_tolerance_warnings.is_empty(),
            "exact-year match must not produce any year_tolerance_warnings; got: {:?}",
            summary.year_tolerance_warnings
        );
        assert_eq!(summary.watched_added, 1);
        assert!(summary.unmatched.is_empty());
    }

    // ── FG-19: bulk-date detection in watched.csv ─────────────────────────────

    fn make_watched_csv(entries: &[(&str, u32, &str, &str)]) -> String {
        // entries: (title, year, slug_suffix, date)
        let mut s = "Date,Name,Year,Letterboxd URI\n".to_string();
        for (title, year, slug, date) in entries {
            s.push_str(&format!(
                "{date},{title},{year},https://letterboxd.com/film/{slug}/\n"
            ));
        }
        s
    }

    #[test]
    fn watched_csv_bulk_date_cluster_detected_and_skipped() {
        // 10 watched.csv films on the same date → bulk day → all skipped, nothing written.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let bulk_date = "2023-06-16";
        let watched = make_watched_csv(&[
            ("Film A", 2001, "film-a", bulk_date),
            ("Film B", 2002, "film-b", bulk_date),
            ("Film C", 2003, "film-c", bulk_date),
            ("Film D", 2004, "film-d", bulk_date),
            ("Film E", 2005, "film-e", bulk_date),
            ("Film F", 2006, "film-f", bulk_date),
            ("Film G", 2007, "film-g", bulk_date),
            ("Film H", 2008, "film-h", bulk_date),
            ("Film I", 2009, "film-i", bulk_date),
            ("Film J", 2010, "film-j", bulk_date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        // Films sorted alphabetically: Film A < Film B < ... < Film J
        let mut gets: Vec<(u16, String)> = (0..10)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push(empty_history());

        let client = MockClient::new(gets, vec![]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "bulk-date cluster must not trigger any write"
        );
        assert_eq!(
            summary.watched_bulk_date_skipped, 10,
            "all 10 bulk-date films must be counted in watched_bulk_date_skipped"
        );
        assert_eq!(summary.watched_added, 0);
        assert_eq!(summary.watched_on_trakt, 0);
        assert_eq!(summary.watched_skipped, 0);
    }

    #[test]
    fn diary_entry_syncs_unaffected_by_bulk_watched_csv_cluster() {
        // Diary entry (real watch date) must sync even when watched.csv has a bulk cluster
        // on the same logged date.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        // Inception in diary with a real watched_date — must sync.
        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2023-06-16,Inception,2010,https://letterboxd.com/film/inception/,,,,2010-07-16\n",
        );

        // 10 OTHER films in watched.csv on the same logged date — bulk, all skipped.
        let bulk_date = "2023-06-16";
        let watched = make_watched_csv(&[
            ("Film A", 2001, "film-a", bulk_date),
            ("Film B", 2002, "film-b", bulk_date),
            ("Film C", 2003, "film-c", bulk_date),
            ("Film D", 2004, "film-d", bulk_date),
            ("Film E", 2005, "film-e", bulk_date),
            ("Film F", 2006, "film-f", bulk_date),
            ("Film G", 2007, "film-g", bulk_date),
            ("Film H", 2008, "film-h", bulk_date),
            ("Film I", 2009, "film-i", bulk_date),
            ("Film J", 2010, "film-j", bulk_date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        // Films sorted: Film A-J then Inception.
        let mut gets: Vec<(u16, String)> = (0..10)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push((200, match_json("Inception", 2010, 123, 27205)));
        gets.push(empty_history());

        let client = MockClient::new(gets, vec![(201, ok_resp(1))]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_added, 1,
            "diary entry must sync despite bulk watched.csv cluster"
        );
        assert_eq!(
            summary.watched_bulk_date_skipped, 10,
            "10 watched.csv bulk-date films must be skipped"
        );

        let bodies = client.post_bodies();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("2010-07-16T00:00:00.000Z")),
            "diary entry must sync with its real watched date"
        );
    }

    #[test]
    fn watched_csv_nine_films_same_date_not_bulk() {
        // 9 films on one day is exactly one below BULK_DATE_THRESHOLD — must NOT be bulk.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let date = "2023-06-16";
        let watched = make_watched_csv(&[
            ("Film A", 2001, "film-a", date),
            ("Film B", 2002, "film-b", date),
            ("Film C", 2003, "film-c", date),
            ("Film D", 2004, "film-d", date),
            ("Film E", 2005, "film-e", date),
            ("Film F", 2006, "film-f", date),
            ("Film G", 2007, "film-g", date),
            ("Film H", 2008, "film-h", date),
            ("Film I", 2009, "film-i", date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        let mut gets: Vec<(u16, String)> = (0..9)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push(empty_history());

        let client = MockClient::new(gets, vec![(201, ok_resp(9))]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_bulk_date_skipped, 0,
            "9 films must NOT trigger bulk-date detection"
        );
        assert_eq!(summary.watched_added, 9, "all 9 films must be added");

        // POST body must contain the shared logged date so the films appear in Trakt with that date.
        let bodies = client.post_bodies();
        assert_eq!(bodies.len(), 1, "exactly one batch POST to sync/history");
        assert!(
            bodies[0].contains("2023-06-16T00:00:00.000Z"),
            "POST body must include the logged date for all 9 non-bulk films; got:\n{}",
            bodies[0]
        );
    }

    #[test]
    fn watched_csv_exactly_ten_films_triggers_bulk() {
        // 10 films on one day hits BULK_DATE_THRESHOLD — all must be skipped.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let date = "2023-06-16";
        let watched = make_watched_csv(&[
            ("Film A", 2001, "film-a", date),
            ("Film B", 2002, "film-b", date),
            ("Film C", 2003, "film-c", date),
            ("Film D", 2004, "film-d", date),
            ("Film E", 2005, "film-e", date),
            ("Film F", 2006, "film-f", date),
            ("Film G", 2007, "film-g", date),
            ("Film H", 2008, "film-h", date),
            ("Film I", 2009, "film-i", date),
            ("Film J", 2010, "film-j", date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        let mut gets: Vec<(u16, String)> = (0..10)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push(empty_history());

        let client = MockClient::new(gets, vec![]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_bulk_date_skipped, 10,
            "10 films must hit BULK_DATE_THRESHOLD and all be skipped"
        );
        assert_eq!(summary.watched_added, 0);
        assert_eq!(client.post_count(), 0, "no writes for bulk-date films");
    }

    #[test]
    fn watched_csv_non_bulk_day_film_still_syncs() {
        // A single watched.csv film on its own date is not a bulk day — must sync.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "watched.csv",
            "Date,Name,Year,Letterboxd URI\n\
            2023-05-15,Inception,2010,https://letterboxd.com/film/inception/\n",
        );

        let client = MockClient::new(
            vec![
                (200, match_json("Inception", 2010, 123, 27205)),
                empty_history(),
            ],
            vec![(201, ok_resp(1))],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_added, 1,
            "non-bulk-day watched.csv film must sync"
        );
        assert_eq!(summary.watched_bulk_date_skipped, 0);
        assert_eq!(client.post_count(), 1);

        // POST body must include the film's logged date so it appears in Trakt history.
        let bodies = client.post_bodies();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("2023-05-15T00:00:00.000Z")),
            "POST body must contain the non-bulk film's logged date as watched_at; got: {:?}",
            bodies
        );
    }

    #[test]
    fn already_on_trakt_check_runs_before_bulk_date_skip() {
        // A film already on Trakt should be counted in watched_on_trakt,
        // even if its date is a bulk day. The already-on-Trakt check runs first.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let bulk_date = "2023-06-16";
        let watched = make_watched_csv(&[
            ("Inception", 2010, "inception", bulk_date),
            ("Film B", 2002, "film-b", bulk_date),
            ("Film C", 2003, "film-c", bulk_date),
            ("Film D", 2004, "film-d", bulk_date),
            ("Film E", 2005, "film-e", bulk_date),
            ("Film F", 2006, "film-f", bulk_date),
            ("Film G", 2007, "film-g", bulk_date),
            ("Film H", 2008, "film-h", bulk_date),
            ("Film I", 2009, "film-i", bulk_date),
            ("Film J", 2010, "film-j", bulk_date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        // Films sorted: Film B-J then Inception.
        let mut gets: Vec<(u16, String)> = (1..10)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push((200, match_json("Inception", 2010, 123, 27205)));
        gets.push(trakt_history_json(&[27205])); // Inception already on Trakt

        let client = MockClient::new(gets, vec![]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_on_trakt, 1,
            "Inception (already on Trakt) must count in watched_on_trakt even on a bulk day"
        );
        assert_eq!(
            summary.watched_bulk_date_skipped, 9,
            "the remaining 9 bulk-day films must be counted in watched_bulk_date_skipped"
        );
        assert_eq!(summary.watched_added, 0);
        assert_eq!(client.post_count(), 0);
    }

    #[test]
    fn bulk_date_threshold_constant_value_is_ten() {
        // BULK_DATE_THRESHOLD lives in constants.rs and is imported by BOTH
        // sync_from_letterboxd (L->T) and sync_to_letterboxd (T->L).
        // Changing it once changes behaviour in both directions.
        assert_eq!(
            crate::constants::BULK_DATE_THRESHOLD,
            10,
            "BULK_DATE_THRESHOLD must be 10 — between a normal film marathon (8-9 films) \
             and known real-world bulk-import clusters"
        );
    }

    #[test]
    fn mixed_bulk_and_non_bulk_days_only_non_bulk_films_appear_in_post() {
        // watched.csv has 1 film on a normal day (non-bulk) and 10 films on a bulk day.
        // Only the non-bulk film must appear in the history POST; the bulk-day films must
        // be absent from every POST body and counted in watched_bulk_date_skipped.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        let non_bulk_date = "2023-01-15";
        let bulk_date = "2023-06-16";

        // 10 films on bulk_date + 1 film on non_bulk_date.
        // Alphabetical order: "Film A"-"Film J" < "Inception" ("F" < "I").
        let watched = make_watched_csv(&[
            ("Film A", 2001, "film-a", bulk_date),
            ("Film B", 2002, "film-b", bulk_date),
            ("Film C", 2003, "film-c", bulk_date),
            ("Film D", 2004, "film-d", bulk_date),
            ("Film E", 2005, "film-e", bulk_date),
            ("Film F", 2006, "film-f", bulk_date),
            ("Film G", 2007, "film-g", bulk_date),
            ("Film H", 2008, "film-h", bulk_date),
            ("Film I", 2009, "film-i", bulk_date),
            ("Film J", 2010, "film-j", bulk_date),
            ("Inception", 2010, "inception", non_bulk_date),
        ]);
        write_csv(&export_dir, "watched.csv", &watched);

        // GET responses: Film A-J (sorted) then Inception.
        let mut gets: Vec<(u16, String)> = (0..10)
            .map(|i| {
                let title = format!("Film {}", (b'A' + i) as char);
                let year = 2001 + i as u32;
                let tmdb = 100 + i as u64;
                (200, match_json(&title, year, tmdb, tmdb))
            })
            .collect();
        gets.push((200, match_json("Inception", 2010, 123, 27205)));
        gets.push(empty_history());

        let client = MockClient::new(gets, vec![(201, ok_resp(1))]);

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            summary.watched_added, 1,
            "only the non-bulk film must be added"
        );
        assert_eq!(
            summary.watched_bulk_date_skipped, 10,
            "all 10 bulk-day films must be counted in watched_bulk_date_skipped"
        );
        assert_eq!(client.post_count(), 1, "exactly one POST to history");

        let bodies = client.post_bodies();
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("2023-01-15T00:00:00.000Z")),
            "POST body must contain the non-bulk film's logged date; got: {:?}",
            bodies
        );
        assert!(
            !bodies.iter().any(|b| b.contains("2023-06-16T00:00:00.000Z")),
            "POST body must NOT contain the bulk-day date — no bulk film should be written to Trakt; got: {:?}",
            bodies
        );
    }

    #[test]
    fn different_title_adjacent_year_goes_to_unmatched_not_year_tolerance_match() {
        // False-positive guard at the integration level:
        // "Original Film" (LB year 2018) — pass 1 returns [], adjacent passes return
        // "Sequel Film" at 2017 and 2019. Different title → must NOT match;
        // must land in unmatched with empty year_tolerance_warnings.
        let export_dir = TempDir::new().unwrap();
        let data_dir = TempDir::new().unwrap();

        write_csv(
            &export_dir,
            "diary.csv",
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
            2024-01-15,Original Film,2018,https://letterboxd.com/film/original-film/,,,,2018-01-15\n",
        );

        let sequel_2017 = match_json("Sequel Film", 2017, 10, 100);
        let sequel_2019 = match_json("Sequel Film", 2019, 11, 101);
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()), // pass 1: years=2018 → no match
                (200, sequel_2017),      // pass 2: years=2017 → wrong title, no match
                (200, sequel_2019),      // pass 2: years=2019 → wrong title, no match
                empty_history(),
            ],
            vec![],
        );

        let summary = run(
            &client,
            data_dir.path(),
            "https://api.trakt.tv",
            "token",
            export_dir.path(),
            false,
            false,
        )
        .unwrap();

        assert_eq!(
            client.post_count(),
            0,
            "different-title adjacent-year must not trigger any write"
        );
        assert_eq!(
            summary.unmatched.len(),
            1,
            "different-title adjacent-year must land in unmatched"
        );
        assert_eq!(summary.unmatched[0].title, "Original Film");
        assert!(
            summary.year_tolerance_warnings.is_empty(),
            "no year_tolerance_warnings for a false-positive adjacency result"
        );
        assert_eq!(summary.watched_added, 0);
    }
}
