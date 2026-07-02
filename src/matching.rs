#![allow(dead_code)]

// Match strategy order (preferred → fallback):
// 1. TmdbId  — use Trakt's /search/tmdb/{id} lookup (exact, fastest)
// 2. ImdbId  — use Trakt's /search/imdb/{id} lookup (exact, no TMDB needed)
// 3. TitleYear — title+year search via /search/movie (Letterboxd exports: no ids)
//
// Letterboxd exports contain no TMDB or IMDb ids, so they always fall through
// to TitleYear. The enum exists so callers that DO have ids (e.g. FG-9 when
// cross-referencing Trakt-side data) can express the preferred strategy.

use crate::trakt_client::TraktHttpClient;
use serde::Deserialize;

pub enum MatchStrategy {
    TmdbId(u64),
    ImdbId(String),
    TitleYear { title: String, year: u32 },
}

#[derive(Debug, Clone)]
pub struct ResolvedIds {
    pub trakt_id: Option<u64>,
    pub tmdb_id: Option<u64>,
    pub imdb_id: Option<String>,
    pub slug: Option<String>,
}

#[derive(Debug)]
pub struct ResolvedFilm {
    pub title: String,
    pub year: u32,
    pub ids: ResolvedIds,
}

#[derive(Debug)]
pub struct UnmatchedFilm {
    pub title: String,
    pub year: u32,
    pub reason: String,
}

#[derive(Deserialize)]
struct SearchResultIds {
    trakt: Option<u64>,
    slug: Option<String>,
    imdb: Option<String>,
    tmdb: Option<u64>,
}

#[derive(Deserialize)]
struct SearchResultMovie {
    title: String,
    year: Option<u32>,
    ids: SearchResultIds,
}

#[derive(Deserialize)]
struct SearchResult {
    movie: SearchResultMovie,
}

/// Percent-encodes a string for use as a URL query parameter value.
/// Leaves unreserved characters (A-Z a-z 0-9 - _ . ~) unencoded;
/// encodes everything else as %XX (including spaces, &, accented chars).
fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// Searches Trakt for a movie by title and year. Returns the best match
/// (exact title + year) or None if no confident match is found.
///
/// Uses GET /search/movie?query=<encoded-title>&years=<year>. The `years`
/// filter narrows results to the given year. Among returned candidates, the
/// first result whose title matches exactly (case-insensitive) at the given
/// year is selected; Trakt orders results by relevance/popularity, so the
/// first exact match is the most-watched / best candidate.
pub fn search_movie_by_title_year(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    title: &str,
    year: u32,
) -> Result<Option<ResolvedIds>, String> {
    let encoded = percent_encode_query(title);
    let url = format!("{base_url}/search/movie?query={encoded}&years={year}");
    let resp = client.get(&url, access_token)?;

    if resp.status != 200 {
        return Err(format!("Trakt search failed: HTTP {}", resp.status));
    }

    let results: Vec<SearchResult> = serde_json::from_str(&resp.body)
        .map_err(|e| format!("failed to parse search results: {e}"))?;

    let title_lower = title.to_lowercase();
    let best = results
        .iter()
        .find(|r| r.movie.title.to_lowercase() == title_lower && r.movie.year == Some(year));

    Ok(best.map(|m| ResolvedIds {
        trakt_id: m.movie.ids.trakt,
        tmdb_id: m.movie.ids.tmdb,
        imdb_id: m.movie.ids.imdb.clone(),
        slug: m.movie.ids.slug.clone(),
    }))
}

/// Resolves a batch of (title, year) pairs against Trakt. Returns a list of
/// successfully resolved films and a list of unmatched films (with reasons).
/// Network errors are returned as Err; "no match" is not an error — it goes
/// into the unmatched list so the caller (FG-9) can report them.
pub fn resolve_films(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    films: &[(String, u32)],
) -> Result<(Vec<ResolvedFilm>, Vec<UnmatchedFilm>), String> {
    let mut matched = Vec::new();
    let mut unmatched = Vec::new();

    for (title, year) in films {
        match search_movie_by_title_year(client, base_url, access_token, title, *year)? {
            Some(ids) => matched.push(ResolvedFilm {
                title: title.clone(),
                year: *year,
                ids,
            }),
            None => unmatched.push(UnmatchedFilm {
                title: title.clone(),
                year: *year,
                reason: "no exact title+year match in Trakt search".to_string(),
            }),
        }
    }

    Ok((matched, unmatched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trakt_client::{HttpResponse, TraktHttpClient};
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    struct MockClient {
        responses: Mutex<VecDeque<(u16, String)>>,
        last_url: Mutex<String>,
    }

    impl MockClient {
        fn new(responses: Vec<(u16, &str)>) -> Self {
            MockClient {
                responses: Mutex::new(
                    responses
                        .into_iter()
                        .map(|(s, b)| (s, b.to_string()))
                        .collect(),
                ),
                last_url: Mutex::new(String::new()),
            }
        }
    }

    impl TraktHttpClient for MockClient {
        fn post_json(&self, _url: &str, _body: &str) -> Result<HttpResponse, String> {
            unreachable!("matching tests do not call post_json")
        }
        fn post_json_auth(
            &self,
            _url: &str,
            _body: &str,
            _token: &str,
        ) -> Result<HttpResponse, String> {
            unreachable!("matching tests do not call post_json_auth")
        }
        fn get(&self, url: &str, _token: &str) -> Result<HttpResponse, String> {
            *self.last_url.lock().unwrap() = url.to_string();
            let mut q = self.responses.lock().unwrap();
            let (status, body) = q
                .pop_front()
                .ok_or_else(|| "no more mock responses".to_string())?;
            Ok(HttpResponse {
                status,
                body,
                headers: HashMap::new(),
            })
        }
    }

    fn one_result(title: &str, year: u32, trakt: u64, tmdb: u64, imdb: &str) -> String {
        format!(
            r#"[{{"type":"movie","score":1000.0,"movie":{{"title":"{title}","year":{year},"ids":{{"trakt":{trakt},"slug":"slug","imdb":"{imdb}","tmdb":{tmdb}}}}}}}]"#
        )
    }

    #[test]
    fn exact_title_year_match_returns_correct_ids() {
        let json = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        let client = MockClient::new(vec![(200, &json)]);
        let ids = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "The Matrix",
            1999,
        )
        .unwrap()
        .expect("should match");
        assert_eq!(ids.tmdb_id, Some(603));
        assert_eq!(ids.trakt_id, Some(481));
        assert_eq!(ids.imdb_id.as_deref(), Some("tt0133093"));
    }

    #[test]
    fn multiple_results_picks_exact_title_year() {
        // Trakt may return partial matches first; we skip to the exact title+year.
        let json = r#"[
            {"type":"movie","score":900.0,"movie":{"title":"The Matrix Reloaded","year":2003,"ids":{"trakt":482,"slug":"s","imdb":"tt0234215","tmdb":604}}},
            {"type":"movie","score":1000.0,"movie":{"title":"The Matrix","year":1999,"ids":{"trakt":481,"slug":"s","imdb":"tt0133093","tmdb":603}}}
        ]"#;
        let client = MockClient::new(vec![(200, json)]);
        let ids = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "The Matrix",
            1999,
        )
        .unwrap()
        .expect("should match");
        assert_eq!(
            ids.tmdb_id,
            Some(603),
            "must pick The Matrix, not The Matrix Reloaded"
        );
    }

    #[test]
    fn no_results_returns_none() {
        let client = MockClient::new(vec![(200, "[]")]);
        let result = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "Nonexistent Film XYZ",
            2099,
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn results_with_no_exact_match_returns_none() {
        // API returns a result but title doesn't match exactly.
        let json = r#"[{"type":"movie","score":500.0,"movie":{"title":"The Matrix Revolutions","year":1999,"ids":{"trakt":483,"slug":"s","imdb":"tt0242653","tmdb":605}}}]"#;
        let client = MockClient::new(vec![(200, json)]);
        let result = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "The Matrix",
            1999,
        )
        .unwrap();
        assert!(
            result.is_none(),
            "non-exact title match should not be accepted"
        );
    }

    #[test]
    fn url_encodes_ampersand_in_title() {
        // "Tom & Jerry": '&' must be encoded as %26 so it doesn't break the query string.
        let json = r#"[{"type":"movie","score":1000.0,"movie":{"title":"Tom & Jerry","year":1992,"ids":{"trakt":1,"slug":"s","imdb":"tt0123","tmdb":999}}}]"#;
        let client = MockClient::new(vec![(200, json)]);
        let ids = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "Tom & Jerry",
            1992,
        )
        .unwrap()
        .expect("should match");
        assert_eq!(ids.tmdb_id, Some(999));
        // Verify the URL contained %26 (encoded &) and not a bare &.
        let url = client.last_url.lock().unwrap().clone();
        assert!(
            url.contains("%26"),
            "& must be percent-encoded in the URL, got: {url}"
        );
        assert!(
            !url.contains("Tom & Jerry"),
            "raw & must not appear in query value, got: {url}"
        );
    }

    #[test]
    fn url_encodes_accented_title() {
        // "Amélie": 'é' is U+00E9, UTF-8 0xC3 0xA9 → %C3%A9.
        let json = r#"[{"type":"movie","score":1000.0,"movie":{"title":"Amélie","year":2001,"ids":{"trakt":123,"slug":"s","imdb":"tt0211915","tmdb":194}}}]"#;
        let client = MockClient::new(vec![(200, json)]);
        let ids =
            search_movie_by_title_year(&client, "https://api.trakt.tv", "token", "Amélie", 2001)
                .unwrap()
                .expect("should match accented title");
        assert_eq!(ids.tmdb_id, Some(194));
        let url = client.last_url.lock().unwrap().clone();
        assert!(
            url.contains("%C3%A9"),
            "é must be percent-encoded as %C3%A9, got: {url}"
        );
    }

    #[test]
    fn non_200_response_returns_error() {
        let client = MockClient::new(vec![(500, r#"{"error":"server error"}"#)]);
        let err =
            search_movie_by_title_year(&client, "https://api.trakt.tv", "token", "Anything", 2000)
                .unwrap_err();
        assert!(err.contains("500"), "error must include status code: {err}");
    }

    #[test]
    fn resolve_films_separates_matched_and_unmatched() {
        let matrix_json = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        // Second film: no results.
        let client = MockClient::new(vec![(200, &matrix_json), (200, "[]")]);
        let films = vec![
            ("The Matrix".to_string(), 1999u32),
            ("Ghost Film XYZ".to_string(), 2050u32),
        ];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].title, "The Matrix");
        assert_eq!(matched[0].ids.tmdb_id, Some(603));
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].title, "Ghost Film XYZ");
        assert!(!unmatched[0].reason.is_empty());
    }

    #[test]
    fn resolve_films_empty_input() {
        let client = MockClient::new(vec![]);
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &[]).unwrap();
        assert!(matched.is_empty());
        assert!(unmatched.is_empty());
    }

    #[test]
    fn percent_encode_query_leaves_unreserved_chars() {
        assert_eq!(
            percent_encode_query("TheMatrix1999-._~"),
            "TheMatrix1999-._~"
        );
    }

    #[test]
    fn percent_encode_query_encodes_space_and_ampersand() {
        assert_eq!(percent_encode_query("Tom & Jerry"), "Tom%20%26%20Jerry");
    }

    // ── Gap coverage (FG-7 verify) ────────────────────────────────────────────

    #[test]
    fn resolve_films_mixed_batch_count_preserved() {
        // Three films: 2 match, 1 doesn't. matched.len() + unmatched.len() must
        // equal input length — no films lost or duplicated.
        let matrix_json = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        let inception_json = one_result("Inception", 2010, 123, 27205, "tt1375666");
        let client = MockClient::new(vec![
            (200, &matrix_json),
            (200, "[]"),
            (200, &inception_json),
        ]);
        let films = vec![
            ("The Matrix".to_string(), 1999u32),
            ("Ghost Film XYZ".to_string(), 2050u32),
            ("Inception".to_string(), 2010u32),
        ];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(
            matched.len() + unmatched.len(),
            films.len(),
            "no films lost or duplicated"
        );
        assert_eq!(matched.len(), 2);
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].title, "Ghost Film XYZ");
    }

    #[test]
    fn year_off_by_one_goes_to_unmatched() {
        // Trakt returns year 2001; we search for 2000. Exact year is required →
        // no match. Documents current strict-year behavior (see notes for ±1-year
        // tolerance tradeoff).
        let json = r#"[{"type":"movie","score":1000.0,"movie":{"title":"Some Film","year":2001,"ids":{"trakt":1,"slug":"s","imdb":"tt0000001","tmdb":1}}}]"#;
        let client = MockClient::new(vec![(200, json)]);
        let result =
            search_movie_by_title_year(&client, "https://api.trakt.tv", "token", "Some Film", 2000)
                .unwrap();
        assert!(
            result.is_none(),
            "off-by-one year must not match — exact year is required"
        );
    }

    #[test]
    fn url_encodes_apostrophe_in_title() {
        // "Schindler's List": apostrophe (0x27) is not an unreserved char and
        // must be encoded as %27 in the query string.
        let json = one_result("Schindler's List", 1993, 77, 424, "tt0108052");
        let client = MockClient::new(vec![(200, &json)]);
        let ids = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "Schindler's List",
            1993,
        )
        .unwrap()
        .expect("should match title containing apostrophe");
        assert_eq!(ids.tmdb_id, Some(424));
        let url = client.last_url.lock().unwrap().clone();
        assert!(
            url.contains("%27"),
            "apostrophe must be percent-encoded as %27, got: {url}"
        );
    }

    #[test]
    fn url_encodes_colon_in_title() {
        // "Amélie: A Story": colon (0x3A) must be encoded as %3A.
        let json = one_result("Amélie: A Story", 2001, 123, 194, "tt0211915");
        let client = MockClient::new(vec![(200, &json)]);
        let ids = search_movie_by_title_year(
            &client,
            "https://api.trakt.tv",
            "token",
            "Amélie: A Story",
            2001,
        )
        .unwrap()
        .expect("should match title containing colon and accented char");
        assert_eq!(ids.tmdb_id, Some(194));
        let url = client.last_url.lock().unwrap().clone();
        assert!(
            url.contains("%3A"),
            "colon must be percent-encoded as %3A, got: {url}"
        );
    }

    #[test]
    fn empty_title_goes_to_unmatched_without_panic() {
        // Current behavior: makes an API call with an empty query param and
        // returns unmatched when no exact title match is found. No panic.
        let client = MockClient::new(vec![(200, "[]")]);
        let films = vec![("".to_string(), 2000u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert!(matched.is_empty());
        assert_eq!(unmatched.len(), 1);
    }

    #[test]
    fn whitespace_only_title_goes_to_unmatched_without_panic() {
        // Whitespace-only title: spaces are encoded (%20) but yield no exact
        // title match → unmatched. No panic.
        let client = MockClient::new(vec![(200, "[]")]);
        let films = vec![("   ".to_string(), 2000u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert!(matched.is_empty());
        assert_eq!(unmatched.len(), 1);
    }

    #[test]
    fn duplicate_input_films_both_resolve_independently() {
        // Two identical (title, year) entries are looked up with separate API
        // calls and both resolve — no silent dedup. Count in == count out.
        let json1 = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        let json2 = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        let client = MockClient::new(vec![(200, &json1), (200, &json2)]);
        let films = vec![
            ("The Matrix".to_string(), 1999u32),
            ("The Matrix".to_string(), 1999u32),
        ];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(
            matched.len() + unmatched.len(),
            films.len(),
            "count in must equal count out"
        );
        assert_eq!(
            matched.len(),
            2,
            "duplicates each resolve independently — no silent dedup"
        );
        assert!(unmatched.is_empty());
    }
}
