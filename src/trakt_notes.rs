use std::collections::HashMap;

use crate::trakt_client::TraktHttpClient;
use serde::{Deserialize, Serialize};

// --- Create note ---

#[derive(Serialize)]
struct AttachedTo {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct NoteIdsPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    tmdb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    imdb: Option<String>,
}

#[derive(Serialize)]
struct NoteMoviePayload {
    ids: NoteIdsPayload,
}

#[derive(Serialize)]
struct CreateNoteBody {
    note: String,
    privacy: &'static str,
    attached_to: AttachedTo,
    movie: NoteMoviePayload,
}

#[derive(Debug)]
pub enum CreateNoteResult {
    Created,
    OverLimit,
}

pub fn create_note(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    text: &str,
    tmdb_id: Option<u64>,
    imdb_id: Option<String>,
) -> Result<CreateNoteResult, String> {
    let body = CreateNoteBody {
        note: text.to_string(),
        privacy: "private",
        attached_to: AttachedTo { kind: "movie" },
        movie: NoteMoviePayload {
            ids: NoteIdsPayload {
                tmdb: tmdb_id,
                imdb: imdb_id,
            },
        },
    };
    let json = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/notes");
    let resp = client.post_json_auth(&url, &json, access_token)?;
    match resp.status {
        201 => Ok(CreateNoteResult::Created),
        402 | 422 => Ok(CreateNoteResult::OverLimit),
        s => Err(format!("unexpected HTTP {s} from POST /notes")),
    }
}

// --- Fetch notes ---

#[derive(Deserialize)]
struct NoteEntry {
    note: String,
    movie: NoteMovieResponse,
}

#[derive(Deserialize)]
struct NoteMovieResponse {
    ids: NoteMovieIds,
}

#[derive(Deserialize)]
struct NoteMovieIds {
    tmdb: Option<u64>,
}

/// Fetch the authenticated user's movie notes. Returns tmdb_id → note text.
/// On any error (network, auth, parse), returns an empty map — best-effort only.
pub fn fetch_movie_notes(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
) -> HashMap<u64, String> {
    let mut map = HashMap::new();
    let mut page = 1u32;

    loop {
        let url = format!("{base_url}/users/me/notes/movies?page={page}&limit=100");
        let resp = match client.get(&url, access_token) {
            Ok(r) => r,
            Err(_) => return map,
        };
        if resp.status != 200 {
            return map;
        }
        let page_count = resp
            .headers
            .get("x-pagination-page-count")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1);

        let entries: Vec<NoteEntry> = match serde_json::from_str(&resp.body) {
            Ok(e) => e,
            Err(_) => return map,
        };

        for entry in entries {
            if let Some(id) = entry.movie.ids.tmdb {
                map.entry(id).or_insert(entry.note);
            }
        }

        if page >= page_count {
            break;
        }
        page += 1;
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trakt_client::{HttpResponse, TraktHttpClient};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    #[allow(clippy::type_complexity)]
    struct MockClient {
        get_responses: Mutex<VecDeque<(u16, String, HashMap<String, String>)>>,
        post_responses: Mutex<VecDeque<(u16, String)>>,
        post_calls: Mutex<Vec<(String, String)>>,
    }

    impl MockClient {
        fn new(get: Vec<(u16, String, HashMap<String, String>)>, post: Vec<(u16, String)>) -> Self {
            MockClient {
                get_responses: Mutex::new(get.into()),
                post_responses: Mutex::new(post.into()),
                post_calls: Mutex::new(Vec::new()),
            }
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
            unreachable!()
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
                .unwrap_or((500, "no more responses".to_string()));
            Ok(HttpResponse {
                status,
                body: resp,
                headers: HashMap::new(),
            })
        }

        fn get(&self, _url: &str, _token: &str) -> Result<HttpResponse, String> {
            let (status, body, headers) = self
                .get_responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "no more mock GET responses".to_string())?;
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

    fn note_entry_json(tmdb: u64, note: &str) -> String {
        format!(
            r#"{{"id":1,"note":"{note}","spoiler":false,"privacy":"private","likes":0,"replies":0,"attached_to":{{"type":"movie","id":1}},"movie":{{"title":"Film","year":2024,"ids":{{"trakt":1,"slug":"film","imdb":"tt1","tmdb":{tmdb}}}}},"created_at":"2024-01-01T00:00:00.000Z","updated_at":"2024-01-01T00:00:00.000Z"}}"#
        )
    }

    #[test]
    fn create_note_success_returns_created() {
        let client = MockClient::new(vec![], vec![(201, r#"{"id":1}"#.to_string())]);
        let result = create_note(
            &client,
            "https://api.trakt.tv",
            "token",
            "Great film",
            Some(603),
            None,
        )
        .unwrap();
        assert!(matches!(result, CreateNoteResult::Created));

        let bodies = client.post_bodies();
        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].contains("\"note\":\"Great film\""));
        assert!(bodies[0].contains("\"type\":\"movie\""));
        assert!(bodies[0].contains("\"tmdb\":603"));
        assert!(bodies[0].contains("\"privacy\":\"private\""));
    }

    #[test]
    fn create_note_402_returns_over_limit() {
        let client = MockClient::new(
            vec![],
            vec![(402, r#"{"error":"payment required"}"#.to_string())],
        );
        let result = create_note(
            &client,
            "https://api.trakt.tv",
            "token",
            "text",
            Some(603),
            None,
        )
        .unwrap();
        assert!(matches!(result, CreateNoteResult::OverLimit));
    }

    #[test]
    fn create_note_422_returns_over_limit() {
        let client = MockClient::new(
            vec![],
            vec![(422, r#"{"error":"limit exceeded"}"#.to_string())],
        );
        let result = create_note(
            &client,
            "https://api.trakt.tv",
            "token",
            "text",
            Some(603),
            None,
        )
        .unwrap();
        assert!(matches!(result, CreateNoteResult::OverLimit));
    }

    #[test]
    fn create_note_unexpected_status_returns_err() {
        let client = MockClient::new(vec![], vec![(500, "error".to_string())]);
        let err = create_note(
            &client,
            "https://api.trakt.tv",
            "token",
            "text",
            Some(603),
            None,
        )
        .unwrap_err();
        assert!(err.contains("500"));
    }

    #[test]
    fn create_note_body_omits_none_ids() {
        let client = MockClient::new(vec![], vec![(201, r#"{"id":1}"#.to_string())]);
        create_note(
            &client,
            "https://api.trakt.tv",
            "token",
            "text",
            None,
            Some("tt1".to_string()),
        )
        .unwrap();
        let bodies = client.post_bodies();
        assert!(
            bodies[0].contains("\"imdb\":\"tt1\""),
            "imdb should be included"
        );
        assert!(
            !bodies[0].contains("\"tmdb\""),
            "tmdb should be omitted when None"
        );
    }

    #[test]
    fn fetch_movie_notes_returns_tmdb_to_text_map() {
        let json = format!("[{}]", note_entry_json(603, "Best film ever"));
        let client = MockClient::new(vec![(200, json, page_headers(1))], vec![]);
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert_eq!(notes.get(&603), Some(&"Best film ever".to_string()));
    }

    #[test]
    fn fetch_movie_notes_empty_returns_empty_map() {
        let client = MockClient::new(vec![(200, "[]".to_string(), page_headers(1))], vec![]);
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert!(notes.is_empty());
    }

    #[test]
    fn fetch_movie_notes_non200_returns_empty_map() {
        let client = MockClient::new(vec![(403, "{}".to_string(), HashMap::new())], vec![]);
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert!(notes.is_empty());
    }

    #[test]
    fn fetch_movie_notes_get_error_returns_empty_map() {
        let client = MockClient::new(vec![], vec![]);
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert!(notes.is_empty());
    }

    #[test]
    fn fetch_movie_notes_pagination_collects_all_pages() {
        let page1 = format!("[{}]", note_entry_json(603, "Note 1"));
        let page2 = format!("[{}]", note_entry_json(27205, "Note 2"));
        let client = MockClient::new(
            vec![(200, page1, page_headers(2)), (200, page2, page_headers(2))],
            vec![],
        );
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes.get(&603), Some(&"Note 1".to_string()));
        assert_eq!(notes.get(&27205), Some(&"Note 2".to_string()));
    }

    #[test]
    fn fetch_movie_notes_entry_without_tmdb_is_skipped() {
        let json = r#"[{"id":1,"note":"text","spoiler":false,"privacy":"private","likes":0,"replies":0,"attached_to":{"type":"movie","id":1},"movie":{"title":"No TMDB","year":2020,"ids":{"trakt":1,"slug":"no-tmdb","imdb":"tt1","tmdb":null}},"created_at":"2024-01-01T00:00:00.000Z","updated_at":"2024-01-01T00:00:00.000Z"}]"#;
        let client = MockClient::new(vec![(200, json.to_string(), page_headers(1))], vec![]);
        let notes = fetch_movie_notes(&client, "https://api.trakt.tv", "token");
        assert!(notes.is_empty(), "entry without tmdb_id must be skipped");
    }
}
