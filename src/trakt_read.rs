// Public types here are consumed by future commands (FG-7/FG-10); suppress premature dead_code warnings.
#![allow(dead_code)]

use crate::trakt_client::{HttpResponse, TraktHttpClient};
use serde::Deserialize;
use std::time::Duration;

// --- Internal API response types ---

#[derive(Deserialize)]
struct MovieIds {
    trakt: Option<u64>,
    slug: Option<String>,
    imdb: Option<String>,
    tmdb: Option<u64>,
}

#[derive(Deserialize)]
struct MovieData {
    title: String,
    year: Option<u32>,
    ids: MovieIds,
}

#[derive(Deserialize)]
struct HistoryEntry {
    watched_at: String,
    movie: MovieData,
}

#[derive(Deserialize)]
struct RatingEntry {
    rated_at: String,
    rating: u8,
    movie: MovieData,
}

#[derive(Deserialize)]
struct WatchlistEntry {
    listed_at: String,
    movie: MovieData,
}

#[derive(Deserialize)]
struct UserSettings {
    user: UserProfile,
}

#[derive(Deserialize)]
struct UserProfile {
    username: String,
}

// --- Public types (reusable by FG-7/FG-10) ---

#[derive(Debug)]
pub struct MovieRecord {
    pub title: String,
    pub year: Option<u32>,
    pub trakt_id: Option<u64>,
    pub slug: Option<String>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<u64>,
}

#[derive(Debug)]
pub struct WatchedMovie {
    pub watched_at: String,
    pub movie: MovieRecord,
}

#[derive(Debug)]
pub struct RatedMovie {
    pub rated_at: String,
    pub rating: u8,
    pub movie: MovieRecord,
}

#[derive(Debug)]
pub struct WatchlistMovie {
    pub listed_at: String,
    pub movie: MovieRecord,
}

// --- Internal helpers ---

fn page_count_from(resp: &HttpResponse) -> u32 {
    resp.headers
        .get("x-pagination-page-count")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(1)
}

fn get_page(
    client: &dyn TraktHttpClient,
    url: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<HttpResponse, String> {
    loop {
        let resp = client.get(url, access_token)?;
        if resp.status == 429 {
            let secs = resp
                .headers
                .get("retry-after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);
            sleep(secs);
        } else {
            return Ok(resp);
        }
    }
}

fn into_record(data: MovieData) -> MovieRecord {
    MovieRecord {
        title: data.title,
        year: data.year,
        trakt_id: data.ids.trakt,
        slug: data.ids.slug,
        imdb_id: data.ids.imdb,
        tmdb_id: data.ids.tmdb,
    }
}

// --- Public functions ---

pub fn fetch_username(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
) -> Result<String, String> {
    fetch_username_inner(client, base_url, access_token, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn fetch_username_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<String, String> {
    let url = format!("{base_url}/users/settings");
    let resp = get_page(client, &url, access_token, sleep)?;
    if resp.status != 200 {
        return Err(format!(
            "failed to fetch user settings: HTTP {}",
            resp.status
        ));
    }
    let settings: UserSettings = serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse user settings: {e}"))?;
    Ok(settings.user.username)
}

pub fn fetch_watched_history(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
) -> Result<Vec<WatchedMovie>, String> {
    fetch_watched_history_inner(client, base_url, access_token, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn fetch_watched_history_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<Vec<WatchedMovie>, String> {
    const LIMIT: u32 = 100;
    let mut all = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!("{base_url}/sync/history/movies?page={page}&limit={LIMIT}");
        let resp = get_page(client, &url, access_token, sleep)?;

        if resp.status != 200 {
            return Err(format!("history fetch failed: HTTP {}", resp.status));
        }

        let page_count = page_count_from(&resp);
        let entries: Vec<HistoryEntry> = serde_json::from_str(&resp.body)
            .map_err(|e| format!("failed to parse history: {e}"))?;

        all.extend(entries.into_iter().map(|e| WatchedMovie {
            watched_at: e.watched_at,
            movie: into_record(e.movie),
        }));

        if page >= page_count {
            break;
        }
        page += 1;
    }

    Ok(all)
}

pub fn fetch_ratings(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
) -> Result<Vec<RatedMovie>, String> {
    fetch_ratings_inner(client, base_url, access_token, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn fetch_ratings_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<Vec<RatedMovie>, String> {
    const LIMIT: u32 = 100;
    let mut all = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!("{base_url}/sync/ratings/movies?page={page}&limit={LIMIT}");
        let resp = get_page(client, &url, access_token, sleep)?;

        if resp.status != 200 {
            return Err(format!("ratings fetch failed: HTTP {}", resp.status));
        }

        let page_count = page_count_from(&resp);
        let entries: Vec<RatingEntry> = serde_json::from_str(&resp.body)
            .map_err(|e| format!("failed to parse ratings: {e}"))?;

        all.extend(entries.into_iter().map(|e| RatedMovie {
            rated_at: e.rated_at,
            rating: e.rating,
            movie: into_record(e.movie),
        }));

        if page >= page_count {
            break;
        }
        page += 1;
    }

    Ok(all)
}

pub fn fetch_watchlist(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
) -> Result<Vec<WatchlistMovie>, String> {
    fetch_watchlist_inner(client, base_url, access_token, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn fetch_watchlist_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<Vec<WatchlistMovie>, String> {
    const LIMIT: u32 = 100;
    let mut all = Vec::new();
    let mut page = 1u32;

    loop {
        let url = format!("{base_url}/sync/watchlist/movies?page={page}&limit={LIMIT}");
        let resp = get_page(client, &url, access_token, sleep)?;

        if resp.status != 200 {
            return Err(format!("watchlist fetch failed: HTTP {}", resp.status));
        }

        let page_count = page_count_from(&resp);
        let entries: Vec<WatchlistEntry> = serde_json::from_str(&resp.body)
            .map_err(|e| format!("failed to parse watchlist: {e}"))?;

        all.extend(entries.into_iter().map(|e| WatchlistMovie {
            listed_at: e.listed_at,
            movie: into_record(e.movie),
        }));

        if page >= page_count {
            break;
        }
        page += 1;
    }

    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trakt_client::{HttpResponse, TraktHttpClient};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

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
            unreachable!("trakt_read tests do not call post_json")
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

    fn no_headers() -> HashMap<String, String> {
        HashMap::new()
    }

    fn history_json(movies: &[(&str, u64)]) -> String {
        let entries: Vec<String> = movies
            .iter()
            .map(|(title, tmdb)| {
                format!(
                    r#"{{"watched_at":"2024-01-01T00:00:00.000Z","movie":{{"title":"{title}","year":2024,"ids":{{"trakt":1,"slug":"slug","imdb":"tt1234567","tmdb":{tmdb}}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    fn rating_json(movies: &[(&str, u8)]) -> String {
        let entries: Vec<String> = movies
            .iter()
            .map(|(title, rating)| {
                format!(
                    r#"{{"rated_at":"2024-01-01T00:00:00.000Z","rating":{rating},"movie":{{"title":"{title}","year":2024,"ids":{{"trakt":1,"slug":"slug","imdb":"tt1234567","tmdb":12345}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    fn watchlist_json(movies: &[&str]) -> String {
        let entries: Vec<String> = movies
            .iter()
            .map(|title| {
                format!(
                    r#"{{"listed_at":"2024-01-01T00:00:00.000Z","movie":{{"title":"{title}","year":2024,"ids":{{"trakt":1,"slug":"slug","imdb":"tt1234567","tmdb":12345}}}}}}"#
                )
            })
            .collect();
        format!("[{}]", entries.join(","))
    }

    #[test]
    fn fetch_watched_history_single_page() {
        let client = MockClient::new(vec![(
            200,
            history_json(&[("The Matrix", 603), ("Inception", 27205)]),
            page_headers(1),
        )]);
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].movie.title, "The Matrix");
        assert_eq!(result[0].movie.tmdb_id, Some(603));
        assert_eq!(result[1].movie.title, "Inception");
    }

    #[test]
    fn fetch_watched_history_multiple_pages() {
        let client = MockClient::new(vec![
            (200, history_json(&[("The Matrix", 603)]), page_headers(2)),
            (200, history_json(&[("Inception", 27205)]), page_headers(2)),
        ]);
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].movie.title, "The Matrix");
        assert_eq!(result[1].movie.title, "Inception");
    }

    #[test]
    fn fetch_watched_history_empty() {
        let client = MockClient::new(vec![(200, "[]".to_string(), page_headers(1))]);
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn fetch_watched_history_retries_on_429() {
        let mut retry_headers = HashMap::new();
        retry_headers.insert("retry-after".to_string(), "2".to_string());
        let client = MockClient::new(vec![
            (429, "{}".to_string(), retry_headers),
            (200, history_json(&[("The Matrix", 603)]), page_headers(1)),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|secs| {
                delays.lock().unwrap().push(secs);
            })
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(*delays.lock().unwrap(), vec![2u64]);
    }

    #[test]
    fn fetch_watched_history_uses_default_retry_delay_when_no_header() {
        let client = MockClient::new(vec![
            (429, "{}".to_string(), no_headers()),
            (200, history_json(&[("Dune", 438631)]), page_headers(1)),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|secs| {
            delays.lock().unwrap().push(secs);
        })
        .unwrap();
        assert_eq!(*delays.lock().unwrap(), vec![5u64]);
    }

    #[test]
    fn fetch_ratings_single_page() {
        let client = MockClient::new(vec![(
            200,
            rating_json(&[("The Matrix", 8), ("Inception", 9)]),
            page_headers(1),
        )]);
        let result =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].movie.title, "The Matrix");
        assert_eq!(result[0].rating, 8);
        assert_eq!(result[1].rating, 9);
    }

    #[test]
    fn fetch_ratings_multiple_pages() {
        let client = MockClient::new(vec![
            (200, rating_json(&[("The Matrix", 8)]), page_headers(2)),
            (200, rating_json(&[("Inception", 9)]), page_headers(2)),
        ]);
        let result =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].movie.title, "The Matrix");
        assert_eq!(result[1].movie.title, "Inception");
    }

    #[test]
    fn fetch_watchlist_single_page() {
        let client = MockClient::new(vec![(
            200,
            watchlist_json(&["Dune", "Blade Runner 2049"]),
            page_headers(1),
        )]);
        let result =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].movie.title, "Dune");
        assert_eq!(result[1].movie.title, "Blade Runner 2049");
    }

    #[test]
    fn fetch_watchlist_multiple_pages() {
        let client = MockClient::new(vec![
            (200, watchlist_json(&["Dune"]), page_headers(2)),
            (200, watchlist_json(&["Blade Runner 2049"]), page_headers(2)),
        ]);
        let result =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn fetch_username_success() {
        let body = r#"{"user":{"username":"stevebargelt","name":"Steve","private":false,"vip":false,"vip_ep":false,"ids":{"slug":"stevebargelt","uuid":"abc123"}}}"#;
        let client = MockClient::new(vec![(200, body.to_string(), no_headers())]);
        let username =
            fetch_username_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(username, "stevebargelt");
    }

    #[test]
    fn fetch_username_http_error() {
        let client = MockClient::new(vec![(403, "{}".to_string(), no_headers())]);
        let err =
            fetch_username_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap_err();
        assert!(err.contains("403"), "expected 403 in error, got: {err}");
    }

    #[test]
    fn movie_ids_captured_correctly() {
        let body = r#"[{"watched_at":"2024-01-01T00:00:00.000Z","movie":{"title":"The Matrix","year":1999,"ids":{"trakt":481,"slug":"the-matrix-1999","imdb":"tt0133093","tmdb":603}}}]"#;
        let client = MockClient::new(vec![(200, body.to_string(), page_headers(1))]);
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        let movie = &result[0].movie;
        assert_eq!(movie.trakt_id, Some(481));
        assert_eq!(movie.slug, Some("the-matrix-1999".to_string()));
        assert_eq!(movie.imdb_id, Some("tt0133093".to_string()));
        assert_eq!(movie.tmdb_id, Some(603));
        assert_eq!(movie.year, Some(1999));
        assert_eq!(result[0].watched_at, "2024-01-01T00:00:00.000Z");
    }

    #[test]
    fn history_error_status_propagates() {
        let client = MockClient::new(vec![(500, "{}".to_string(), no_headers())]);
        let err = fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {})
            .unwrap_err();
        assert!(err.contains("500"), "expected 500 in error, got: {err}");
    }

    #[test]
    fn ratings_error_status_propagates() {
        let client = MockClient::new(vec![(401, "{}".to_string(), no_headers())]);
        let err =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap_err();
        assert!(err.contains("401"), "expected 401 in error, got: {err}");
    }

    #[test]
    fn watchlist_error_status_propagates() {
        let client = MockClient::new(vec![(401, "{}".to_string(), no_headers())]);
        let err =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap_err();
        assert!(err.contains("401"), "expected 401 in error, got: {err}");
    }

    // --- New gap-coverage tests ---

    #[test]
    fn fetch_watched_history_three_pages() {
        let client = MockClient::new(vec![
            (200, history_json(&[("Movie A", 1)]), page_headers(3)),
            (200, history_json(&[("Movie B", 2)]), page_headers(3)),
            (200, history_json(&[("Movie C", 3)]), page_headers(3)),
        ]);
        let result =
            fetch_watched_history_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].movie.title, "Movie A");
        assert_eq!(result[1].movie.title, "Movie B");
        assert_eq!(result[2].movie.title, "Movie C");
    }

    #[test]
    fn fetch_ratings_three_pages() {
        let client = MockClient::new(vec![
            (200, rating_json(&[("Movie A", 7)]), page_headers(3)),
            (200, rating_json(&[("Movie B", 8)]), page_headers(3)),
            (200, rating_json(&[("Movie C", 9)]), page_headers(3)),
        ]);
        let result =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].movie.title, "Movie A");
        assert_eq!(result[1].movie.title, "Movie B");
        assert_eq!(result[2].movie.title, "Movie C");
    }

    #[test]
    fn fetch_watchlist_three_pages() {
        let client = MockClient::new(vec![
            (200, watchlist_json(&["Movie A"]), page_headers(3)),
            (200, watchlist_json(&["Movie B"]), page_headers(3)),
            (200, watchlist_json(&["Movie C"]), page_headers(3)),
        ]);
        let result =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].movie.title, "Movie A");
        assert_eq!(result[1].movie.title, "Movie B");
        assert_eq!(result[2].movie.title, "Movie C");
    }

    #[test]
    fn fetch_ratings_retries_on_429() {
        let mut retry_headers = HashMap::new();
        retry_headers.insert("retry-after".to_string(), "3".to_string());
        let client = MockClient::new(vec![
            (429, "{}".to_string(), retry_headers),
            (200, rating_json(&[("Inception", 9)]), page_headers(1)),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let result = fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|secs| {
            delays.lock().unwrap().push(secs);
        })
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(*delays.lock().unwrap(), vec![3u64]);
    }

    #[test]
    fn fetch_watchlist_retries_on_429() {
        let mut retry_headers = HashMap::new();
        retry_headers.insert("retry-after".to_string(), "1".to_string());
        let client = MockClient::new(vec![
            (429, "{}".to_string(), retry_headers),
            (200, watchlist_json(&["Dune"]), page_headers(1)),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let result = fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|secs| {
            delays.lock().unwrap().push(secs);
        })
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn fetch_username_retries_on_429() {
        let mut retry_headers = HashMap::new();
        retry_headers.insert("retry-after".to_string(), "2".to_string());
        let body = r#"{"user":{"username":"testuser","name":"Test","private":false,"vip":false,"vip_ep":false,"ids":{"slug":"testuser","uuid":"abc123"}}}"#;
        let client = MockClient::new(vec![
            (429, "{}".to_string(), retry_headers),
            (200, body.to_string(), no_headers()),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let username = fetch_username_inner(&client, "https://api.trakt.tv", "token", &|secs| {
            delays.lock().unwrap().push(secs);
        })
        .unwrap();
        assert_eq!(username, "testuser");
        assert_eq!(*delays.lock().unwrap(), vec![2u64]);
    }

    #[test]
    fn ratings_ids_and_rated_at_captured() {
        let body = r#"[{"rated_at":"2024-06-15T12:00:00.000Z","rating":8,"movie":{"title":"Inception","year":2010,"ids":{"trakt":28,"slug":"inception-2010","imdb":"tt1375666","tmdb":27205}}}]"#;
        let client = MockClient::new(vec![(200, body.to_string(), page_headers(1))]);
        let result =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        let entry = &result[0];
        assert_eq!(entry.rated_at, "2024-06-15T12:00:00.000Z");
        assert_eq!(entry.rating, 8);
        assert_eq!(entry.movie.title, "Inception");
        assert_eq!(entry.movie.trakt_id, Some(28));
        assert_eq!(entry.movie.slug, Some("inception-2010".to_string()));
        assert_eq!(entry.movie.imdb_id, Some("tt1375666".to_string()));
        assert_eq!(entry.movie.tmdb_id, Some(27205));
        assert_eq!(entry.movie.year, Some(2010));
    }

    #[test]
    fn watchlist_ids_and_listed_at_captured() {
        let body = r#"[{"listed_at":"2024-03-20T08:30:00.000Z","movie":{"title":"Blade Runner 2049","year":2017,"ids":{"trakt":170,"slug":"blade-runner-2049","imdb":"tt1856101","tmdb":335984}}}]"#;
        let client = MockClient::new(vec![(200, body.to_string(), page_headers(1))]);
        let result =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        let entry = &result[0];
        assert_eq!(entry.listed_at, "2024-03-20T08:30:00.000Z");
        assert_eq!(entry.movie.title, "Blade Runner 2049");
        assert_eq!(entry.movie.trakt_id, Some(170));
        assert_eq!(entry.movie.slug, Some("blade-runner-2049".to_string()));
        assert_eq!(entry.movie.imdb_id, Some("tt1856101".to_string()));
        assert_eq!(entry.movie.tmdb_id, Some(335984));
        assert_eq!(entry.movie.year, Some(2017));
    }

    #[test]
    fn fetch_ratings_empty() {
        let client = MockClient::new(vec![(200, "[]".to_string(), page_headers(1))]);
        let result =
            fetch_ratings_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn fetch_watchlist_empty() {
        let client = MockClient::new(vec![(200, "[]".to_string(), page_headers(1))]);
        let result =
            fetch_watchlist_inner(&client, "https://api.trakt.tv", "token", &|_| {}).unwrap();
        assert_eq!(result.len(), 0);
    }
}
