use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::{
    letterboxd_export::LetterboxdExport,
    matching::{resolve_films, UnmatchedFilm},
    rating::letterboxd_rating_to_trakt,
    sync_state::{Direction, ItemRef, ItemType, SyncKey, SyncState},
    trakt_client::TraktHttpClient,
    trakt_notes::{self, CreateNoteResult},
    trakt_write::{self, HistoryMovie, MovieId, RatingMovie, WatchlistMovie},
};

pub struct ErroredItem {
    pub title: String,
    pub year: u32,
    pub reason: String,
}

pub struct SyncSummary {
    pub watched_added: u32,
    pub watched_skipped: u32,
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

    let mut watched_skipped = 0u32;
    let mut ratings_skipped = 0u32;
    let mut watchlist_skipped = 0u32;
    let mut errored: Vec<ErroredItem> = Vec::new();

    // --- History ---
    // Parallel vecs: movies to write and their key components for state marking.
    let mut history_movies: Vec<HistoryMovie> = Vec::new();
    let mut history_keys: Vec<(Option<u64>, String, u32, String)> = Vec::new();

    // Diary slugs guard against re-adding watched.csv entries already in diary.
    let diary_slugs: HashSet<&str> = export.diary.iter().map(|e| e.slug.as_str()).collect();

    for entry in &export.diary {
        let (tmdb_id, imdb_id) = match lookup(&entry.name, entry.year) {
            Some(ids) => ids,
            None => continue,
        };
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
        watched_skipped,
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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

        // No match for The Matrix.
        let client = MockClient::new(vec![(200, "[]".to_string())], vec![]);

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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("Inception", 2010, 123, 27205))],
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

        let client = MockClient::new(vec![(200, match_json("Dune", 2021, 999, 438631))], vec![]);

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
        let trakt_resp = r#"[{"type":"movie","score":1000.0,"movie":{"title":"Amélie","year":2001,"ids":{"trakt":123,"slug":"amelie","imdb":"tt0211915","tmdb":194}}}]"#;
        let client = MockClient::new(vec![(200, trakt_resp.to_string())], vec![]);

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

        // GET responses in sorted resolve order: Ghost Film → [] , Inception → match, The Matrix → match.
        let client = MockClient::new(
            vec![
                (200, "[]".to_string()),
                (200, match_json("Inception", 2010, 123, 27205)),
                (200, match_json("The Matrix", 1999, 481, 603)),
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

        let client = MockClient::new(vec![], vec![]);

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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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

        // No match for Unknown Film
        let client = MockClient::new(vec![(200, "[]".to_string())], vec![]);

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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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
            vec![(200, match_json("The Matrix", 1999, 481, 603))],
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

        let client = MockClient::new(vec![(200, "[]".to_string())], vec![]);

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
}
