// Public types here are consumed by FG-9; suppress premature dead_code warnings.
#![allow(dead_code)]

use crate::trakt_client::TraktHttpClient;
use serde::{Deserialize, Serialize};
use std::time::Duration;

// --- Public input types ---

pub struct MovieId {
    pub tmdb_id: Option<u64>,
    pub imdb_id: Option<String>,
}

pub struct HistoryMovie {
    pub ids: MovieId,
    pub watched_at: Option<String>,
}

pub struct RatingMovie {
    pub ids: MovieId,
    pub rating: u8,
    pub rated_at: Option<String>,
}

pub struct WatchlistMovie {
    pub ids: MovieId,
}

// --- Public output type ---

#[derive(Debug, PartialEq)]
pub struct SyncSummary {
    /// Items added (or deleted for remove operations).
    pub added: u32,
    /// Items updated or already existing (0 for remove operations).
    pub updated: u32,
    /// Items Trakt could not find by the provided IDs.
    pub not_found: u32,
}

// --- Request payload types ---

#[derive(Serialize)]
struct IdsPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    tmdb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    imdb: Option<String>,
}

#[derive(Serialize)]
struct HistoryMoviePayload {
    ids: IdsPayload,
    #[serde(skip_serializing_if = "Option::is_none")]
    watched_at: Option<String>,
}

#[derive(Serialize)]
struct HistoryBody {
    movies: Vec<HistoryMoviePayload>,
}

#[derive(Serialize)]
struct RatingMoviePayload {
    ids: IdsPayload,
    rating: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    rated_at: Option<String>,
}

#[derive(Serialize)]
struct RatingsBody {
    movies: Vec<RatingMoviePayload>,
}

#[derive(Serialize)]
struct WatchlistMoviePayload {
    ids: IdsPayload,
}

#[derive(Serialize)]
struct WatchlistBody {
    movies: Vec<WatchlistMoviePayload>,
}

// --- Response payload types ---

#[derive(Deserialize, Default)]
struct MovieCount {
    movies: Option<u32>,
}

#[derive(Deserialize, Default)]
struct NotFoundBlock {
    movies: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize, Default)]
struct AddSyncResponse {
    #[serde(default)]
    added: MovieCount,
    #[serde(default)]
    updated: MovieCount,
    #[serde(default)]
    existing: MovieCount,
    #[serde(default)]
    not_found: NotFoundBlock,
}

#[derive(Deserialize, Default)]
struct RemoveSyncResponse {
    #[serde(default)]
    deleted: MovieCount,
    #[serde(default)]
    not_found: NotFoundBlock,
}

// --- Internal helpers ---

fn build_ids(id: &MovieId) -> IdsPayload {
    if id.tmdb_id.is_some() {
        IdsPayload {
            tmdb: id.tmdb_id,
            imdb: None,
        }
    } else {
        IdsPayload {
            tmdb: None,
            imdb: id.imdb_id.clone(),
        }
    }
}

fn not_found_count(block: NotFoundBlock) -> u32 {
    block.movies.map(|v| v.len() as u32).unwrap_or(0)
}

fn post_with_rate_limit(
    client: &dyn TraktHttpClient,
    url: &str,
    body: &str,
    access_token: &str,
    sleep: &dyn Fn(u64),
) -> Result<String, String> {
    loop {
        let resp = client.post_json_auth(url, body, access_token)?;
        match resp.status {
            200 | 201 => {
                // Enforce >=1s between successive POSTs before returning to caller.
                sleep(1);
                return Ok(resp.body);
            }
            429 => {
                let secs = resp
                    .headers
                    .get("retry-after")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1);
                sleep(secs);
            }
            s => return Err(format!("unexpected HTTP {s}")),
        }
    }
}

// --- Public functions ---

pub fn add_to_history(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[HistoryMovie],
) -> Result<SyncSummary, String> {
    add_to_history_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn add_to_history_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[HistoryMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = HistoryBody {
        movies: movies
            .iter()
            .map(|m| HistoryMoviePayload {
                ids: build_ids(&m.ids),
                watched_at: m.watched_at.clone(),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/history");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: AddSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse history response: {e}"))?;
    Ok(SyncSummary {
        added: resp.added.movies.unwrap_or(0),
        updated: resp.existing.movies.unwrap_or(0),
        not_found: not_found_count(resp.not_found),
    })
}

pub fn remove_from_history(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[HistoryMovie],
) -> Result<SyncSummary, String> {
    remove_from_history_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn remove_from_history_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[HistoryMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = HistoryBody {
        movies: movies
            .iter()
            .map(|m| HistoryMoviePayload {
                ids: build_ids(&m.ids),
                watched_at: m.watched_at.clone(),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/history/remove");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: RemoveSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse history/remove response: {e}"))?;
    Ok(SyncSummary {
        added: resp.deleted.movies.unwrap_or(0),
        updated: 0,
        not_found: not_found_count(resp.not_found),
    })
}

pub fn add_ratings(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[RatingMovie],
) -> Result<SyncSummary, String> {
    add_ratings_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn add_ratings_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[RatingMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = RatingsBody {
        movies: movies
            .iter()
            .map(|m| RatingMoviePayload {
                ids: build_ids(&m.ids),
                rating: m.rating,
                rated_at: m.rated_at.clone(),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/ratings");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: AddSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse ratings response: {e}"))?;
    Ok(SyncSummary {
        added: resp.added.movies.unwrap_or(0),
        updated: resp.updated.movies.unwrap_or(0),
        not_found: not_found_count(resp.not_found),
    })
}

pub fn remove_ratings(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[RatingMovie],
) -> Result<SyncSummary, String> {
    remove_ratings_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn remove_ratings_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[RatingMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = RatingsBody {
        movies: movies
            .iter()
            .map(|m| RatingMoviePayload {
                ids: build_ids(&m.ids),
                rating: m.rating,
                rated_at: m.rated_at.clone(),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/ratings/remove");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: RemoveSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse ratings/remove response: {e}"))?;
    Ok(SyncSummary {
        added: resp.deleted.movies.unwrap_or(0),
        updated: 0,
        not_found: not_found_count(resp.not_found),
    })
}

pub fn add_to_watchlist(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[WatchlistMovie],
) -> Result<SyncSummary, String> {
    add_to_watchlist_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn add_to_watchlist_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[WatchlistMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = WatchlistBody {
        movies: movies
            .iter()
            .map(|m| WatchlistMoviePayload {
                ids: build_ids(&m.ids),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/watchlist");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: AddSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse watchlist response: {e}"))?;
    Ok(SyncSummary {
        added: resp.added.movies.unwrap_or(0),
        updated: resp.existing.movies.unwrap_or(0),
        not_found: not_found_count(resp.not_found),
    })
}

pub fn remove_from_watchlist(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[WatchlistMovie],
) -> Result<SyncSummary, String> {
    remove_from_watchlist_inner(client, base_url, access_token, movies, &|secs| {
        std::thread::sleep(Duration::from_secs(secs))
    })
}

fn remove_from_watchlist_inner(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    movies: &[WatchlistMovie],
    sleep: &dyn Fn(u64),
) -> Result<SyncSummary, String> {
    let body = WatchlistBody {
        movies: movies
            .iter()
            .map(|m| WatchlistMoviePayload {
                ids: build_ids(&m.ids),
            })
            .collect(),
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/sync/watchlist/remove");
    let resp_body = post_with_rate_limit(client, &url, &json, access_token, sleep)?;
    let resp: RemoveSyncResponse = serde_json::from_str(&resp_body)
        .map_err(|e| format!("failed to parse watchlist/remove response: {e}"))?;
    Ok(SyncSummary {
        added: resp.deleted.movies.unwrap_or(0),
        updated: 0,
        not_found: not_found_count(resp.not_found),
    })
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
        captured_bodies: Mutex<Vec<String>>,
    }

    impl MockClient {
        fn new(responses: Vec<(u16, String, HashMap<String, String>)>) -> Self {
            MockClient {
                responses: Mutex::new(responses.into()),
                captured_bodies: Mutex::new(Vec::new()),
            }
        }

        fn last_body(&self) -> String {
            self.captured_bodies
                .lock()
                .unwrap()
                .last()
                .cloned()
                .unwrap_or_default()
        }
    }

    impl TraktHttpClient for MockClient {
        fn post_json(&self, _url: &str, _body: &str) -> Result<HttpResponse, String> {
            unreachable!("trakt_write tests do not call post_json")
        }

        fn post_json_auth(
            &self,
            _url: &str,
            body: &str,
            _access_token: &str,
        ) -> Result<HttpResponse, String> {
            self.captured_bodies.lock().unwrap().push(body.to_string());
            let mut q = self.responses.lock().unwrap();
            let (status, resp_body, headers) = q
                .pop_front()
                .ok_or_else(|| "no more mock responses".to_string())?;
            Ok(HttpResponse {
                status,
                body: resp_body,
                headers,
            })
        }

        fn get(&self, _url: &str, _access_token: &str) -> Result<HttpResponse, String> {
            unreachable!("trakt_write tests do not call get")
        }
    }

    fn no_headers() -> HashMap<String, String> {
        HashMap::new()
    }

    fn retry_headers(secs: u64) -> HashMap<String, String> {
        let mut h = HashMap::new();
        h.insert("retry-after".to_string(), secs.to_string());
        h
    }

    fn history_add_resp(added: u32, not_found: usize) -> String {
        let nf: Vec<String> = (0..not_found)
            .map(|i| format!(r#"{{"ids":{{"tmdb":{i}}}}}"#))
            .collect();
        format!(
            r#"{{"added":{{"movies":{added},"episodes":0}},"not_found":{{"movies":[{}]}}}}"#,
            nf.join(",")
        )
    }

    fn history_remove_resp(deleted: u32, not_found: usize) -> String {
        let nf: Vec<String> = (0..not_found)
            .map(|i| format!(r#"{{"ids":{{"tmdb":{i}}}}}"#))
            .collect();
        format!(
            r#"{{"deleted":{{"movies":{deleted},"episodes":0}},"not_found":{{"movies":[{}]}}}}"#,
            nf.join(",")
        )
    }

    fn ratings_add_resp(added: u32, updated: u32) -> String {
        format!(
            r#"{{"added":{{"movies":{added}}},"updated":{{"movies":{updated}}},"not_found":{{"movies":[]}}}}"#
        )
    }

    fn watchlist_add_resp(added: u32, existing: u32) -> String {
        format!(
            r#"{{"added":{{"movies":{added}}},"existing":{{"movies":{existing}}},"not_found":{{"movies":[]}}}}"#
        )
    }

    fn history_movie(tmdb: u64, watched_at: Option<&str>) -> HistoryMovie {
        HistoryMovie {
            ids: MovieId {
                tmdb_id: Some(tmdb),
                imdb_id: None,
            },
            watched_at: watched_at.map(str::to_owned),
        }
    }

    fn rating_movie(tmdb: u64, rating: u8) -> RatingMovie {
        RatingMovie {
            ids: MovieId {
                tmdb_id: Some(tmdb),
                imdb_id: None,
            },
            rating,
            rated_at: None,
        }
    }

    fn watchlist_movie(tmdb: u64) -> WatchlistMovie {
        WatchlistMovie {
            ids: MovieId {
                tmdb_id: Some(tmdb),
                imdb_id: None,
            },
        }
    }

    #[test]
    fn add_history_success() {
        let client = MockClient::new(vec![(201, history_add_resp(2, 0), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![
            history_movie(603, Some("2024-01-01T00:00:00.000Z")),
            history_movie(27205, Some("2024-02-01T00:00:00.000Z")),
        ];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.not_found, 0);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn remove_history_success() {
        let client = MockClient::new(vec![(200, history_remove_resp(1, 1), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![history_movie(603, None)];
        let summary =
            remove_from_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.not_found, 1);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn add_ratings_success() {
        let client = MockClient::new(vec![(200, ratings_add_resp(1, 1), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![
            RatingMovie {
                ids: MovieId {
                    tmdb_id: Some(603),
                    imdb_id: None,
                },
                rating: 8,
                rated_at: Some("2024-01-01T00:00:00.000Z".to_string()),
            },
            rating_movie(27205, 9),
        ];
        let summary = add_ratings_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
            delays.lock().unwrap().push(s)
        })
        .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(summary.updated, 1);
        assert_eq!(summary.not_found, 0);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn remove_ratings_success() {
        let client = MockClient::new(vec![(200, history_remove_resp(2, 0), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![rating_movie(603, 8), rating_movie(27205, 9)];
        let summary =
            remove_ratings_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.not_found, 0);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn add_watchlist_success() {
        let client = MockClient::new(vec![(200, watchlist_add_resp(2, 1), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![watchlist_movie(603), watchlist_movie(27205)];
        let summary =
            add_to_watchlist_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(summary.updated, 1);
        assert_eq!(summary.not_found, 0);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn remove_watchlist_success() {
        let client = MockClient::new(vec![(200, history_remove_resp(1, 0), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![watchlist_movie(603)];
        let summary =
            remove_from_watchlist_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.not_found, 0);
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn rate_limit_retry_on_429_with_retry_after() {
        let client = MockClient::new(vec![
            (429, "{}".to_string(), retry_headers(3)),
            (200, history_add_resp(1, 0), no_headers()),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![history_movie(603, None)];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
                delays.lock().unwrap().push(s)
            })
            .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(*delays.lock().unwrap(), vec![3u64, 1u64]);
    }

    #[test]
    fn rate_limit_retry_on_429_default_delay() {
        let client = MockClient::new(vec![
            (429, "{}".to_string(), no_headers()),
            (200, history_add_resp(1, 0), no_headers()),
        ]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![history_movie(603, None)];
        add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
            delays.lock().unwrap().push(s)
        })
        .unwrap();
        assert_eq!(*delays.lock().unwrap(), vec![1u64, 1u64]);
    }

    #[test]
    fn rate_limit_sleep_called_after_success() {
        let client = MockClient::new(vec![(200, history_add_resp(1, 0), no_headers())]);
        let delays: Mutex<Vec<u64>> = Mutex::new(Vec::new());
        let movies = vec![history_movie(603, None)];
        add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|s| {
            delays.lock().unwrap().push(s)
        })
        .unwrap();
        assert_eq!(*delays.lock().unwrap(), vec![1u64]);
    }

    #[test]
    fn id_prefers_tmdb_over_imdb() {
        let client = MockClient::new(vec![(200, history_add_resp(1, 0), no_headers())]);
        let movies = vec![HistoryMovie {
            ids: MovieId {
                tmdb_id: Some(603),
                imdb_id: Some("tt0133093".to_string()),
            },
            watched_at: None,
        }];
        add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            body.contains("\"tmdb\":603"),
            "expected tmdb id in body: {body}"
        );
        assert!(
            !body.contains("\"imdb\""),
            "unexpected imdb field in body: {body}"
        );
    }

    #[test]
    fn id_falls_back_to_imdb() {
        let client = MockClient::new(vec![(200, history_add_resp(1, 0), no_headers())]);
        let movies = vec![HistoryMovie {
            ids: MovieId {
                tmdb_id: None,
                imdb_id: Some("tt0133093".to_string()),
            },
            watched_at: None,
        }];
        add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            body.contains("\"imdb\":\"tt0133093\""),
            "expected imdb id in body: {body}"
        );
        assert!(
            !body.contains("\"tmdb\""),
            "unexpected tmdb field in body: {body}"
        );
    }

    #[test]
    fn response_summary_not_found_count() {
        let resp = r#"{"added":{"movies":1},"not_found":{"movies":[{"ids":{"tmdb":999}},{"ids":{"tmdb":888}}]}}"#;
        let client = MockClient::new(vec![(200, resp.to_string(), no_headers())]);
        let movies = vec![history_movie(603, None)];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(summary.not_found, 2);
    }

    #[test]
    fn history_watched_at_included_in_body() {
        let client = MockClient::new(vec![(200, history_add_resp(1, 0), no_headers())]);
        let movies = vec![history_movie(603, Some("2023-11-15T20:00:00.000Z"))];
        add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            body.contains("2023-11-15T20:00:00.000Z"),
            "expected watched_at in body: {body}"
        );
    }

    #[test]
    fn ratings_value_included_in_body() {
        let client = MockClient::new(vec![(200, ratings_add_resp(1, 0), no_headers())]);
        let movies = vec![rating_movie(603, 7)];
        add_ratings_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            body.contains("\"rating\":7"),
            "expected rating in body: {body}"
        );
    }

    #[test]
    fn unexpected_http_status_returns_error() {
        let client = MockClient::new(vec![(500, "internal error".to_string(), no_headers())]);
        let movies = vec![history_movie(603, None)];
        let err = add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
            .unwrap_err();
        assert!(err.contains("500"), "expected 500 in error: {err}");
    }

    // Gap 1: multiple movies in one call — body contains all entries, summary sums correctly.
    #[test]
    fn add_history_batch_serializes_all_movies() {
        let resp = r#"{"added":{"movies":3},"not_found":{"movies":[]}}"#;
        let client = MockClient::new(vec![(201, resp.to_string(), no_headers())]);
        let movies = vec![
            history_movie(603, Some("2024-01-01T00:00:00.000Z")),
            history_movie(27205, Some("2024-02-01T00:00:00.000Z")),
            history_movie(550, Some("2024-03-01T00:00:00.000Z")),
        ];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 3);
        assert_eq!(summary.not_found, 0);
        let body = client.last_body();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let arr = parsed["movies"].as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["ids"]["tmdb"], 603);
        assert_eq!(arr[1]["ids"]["tmdb"], 27205);
        assert_eq!(arr[2]["ids"]["tmdb"], 550);
    }

    // Gap 2: empty input list — must not crash; sends empty array and returns zero summary.
    #[test]
    fn add_history_empty_input_sends_empty_array() {
        let resp = r#"{"added":{"movies":0},"not_found":{"movies":[]}}"#;
        let client = MockClient::new(vec![(200, resp.to_string(), no_headers())]);
        let movies: Vec<HistoryMovie> = vec![];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 0);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.not_found, 0);
        let body = client.last_body();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["movies"].as_array().unwrap().len(), 0);
    }

    // Gap 3a: 'existing' field in Trakt add-history response maps to SyncSummary.updated.
    #[test]
    fn existing_count_maps_to_updated_for_history_add() {
        let resp = r#"{"added":{"movies":2},"existing":{"movies":3},"not_found":{"movies":[]}}"#;
        let client = MockClient::new(vec![(200, resp.to_string(), no_headers())]);
        let movies = vec![history_movie(603, None)];
        let summary =
            add_to_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 2);
        assert_eq!(
            summary.updated, 3,
            "'existing' from Trakt must map to SyncSummary.updated"
        );
        assert_eq!(summary.not_found, 0);
    }

    // Gap 3b: same 'existing' mapping for add-watchlist.
    #[test]
    fn existing_count_maps_to_updated_for_watchlist_add() {
        let resp = r#"{"added":{"movies":1},"existing":{"movies":4},"not_found":{"movies":[]}}"#;
        let client = MockClient::new(vec![(200, resp.to_string(), no_headers())]);
        let movies = vec![watchlist_movie(603)];
        let summary =
            add_to_watchlist_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 1);
        assert_eq!(
            summary.updated, 4,
            "'existing' from Trakt must map to SyncSummary.updated"
        );
        assert_eq!(summary.not_found, 0);
    }

    // Gap 4a: rated_at is serialized into body when Some.
    #[test]
    fn ratings_rated_at_included_in_body_when_some() {
        let client = MockClient::new(vec![(200, ratings_add_resp(1, 0), no_headers())]);
        let movies = vec![RatingMovie {
            ids: MovieId {
                tmdb_id: Some(603),
                imdb_id: None,
            },
            rating: 8,
            rated_at: Some("2024-06-15T12:00:00.000Z".to_string()),
        }];
        add_ratings_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            body.contains("\"rated_at\""),
            "expected rated_at key in body: {body}"
        );
        assert!(
            body.contains("2024-06-15T12:00:00.000Z"),
            "expected rated_at value in body: {body}"
        );
    }

    // Gap 4b: rated_at is omitted from body when None (skip_serializing_if).
    #[test]
    fn ratings_rated_at_absent_from_body_when_none() {
        let client = MockClient::new(vec![(200, ratings_add_resp(1, 0), no_headers())]);
        let movies = vec![rating_movie(603, 8)];
        add_ratings_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {}).unwrap();
        let body = client.last_body();
        assert!(
            !body.contains("rated_at"),
            "expected rated_at absent from body when None: {body}"
        );
    }

    // Gap 5: remove response — 'deleted' maps to SyncSummary.added, 'not_found' is counted,
    // and SyncSummary.updated is always 0 for remove operations.
    #[test]
    fn remove_history_response_maps_deleted_and_not_found() {
        let resp = r#"{"deleted":{"movies":3},"not_found":{"movies":[{"ids":{"tmdb":999}},{"ids":{"tmdb":888}}]}}"#;
        let client = MockClient::new(vec![(200, resp.to_string(), no_headers())]);
        let movies = vec![history_movie(603, None)];
        let summary =
            remove_from_history_inner(&client, "https://api.trakt.tv", "token", &movies, &|_| {})
                .unwrap();
        assert_eq!(summary.added, 3, "'deleted' must map to SyncSummary.added");
        assert_eq!(
            summary.updated, 0,
            "remove operations always yield updated=0"
        );
        assert_eq!(
            summary.not_found, 2,
            "not_found count must match array length"
        );
    }
}
