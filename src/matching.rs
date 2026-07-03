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
    /// Set when the film was matched by ±1 year tolerance rather than exact year.
    pub year_tolerance_warning: Option<String>,
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

/// For a film that failed exact-year search, tries year-1 then year+1 (same
/// title required). Returns `Some((ids, matched_year))` on the first hit.
fn try_adjacent_year_match(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    title: &str,
    year: u32,
) -> Result<Option<(ResolvedIds, u32)>, String> {
    let adj_years: Vec<u32> = {
        let mut v = Vec::with_capacity(2);
        if year > 0 {
            v.push(year - 1);
        }
        v.push(year + 1);
        v
    };
    for adj in adj_years {
        if let Some(ids) = search_movie_by_title_year(client, base_url, access_token, title, adj)? {
            return Ok(Some((ids, adj)));
        }
    }
    Ok(None)
}

/// Resolves a batch of (title, year) pairs against Trakt. Returns a list of
/// successfully resolved films and a list of unmatched films (with reasons).
/// Network errors are returned as Err; "no match" is not an error — it goes
/// into the unmatched list so the caller (FG-9) can report them.
///
/// Two-pass strategy:
///   1. Exact title+year match (authoritative).
///   2. For films with no exact match, retry with year ±1 on an identical
///      (case-normalized) title. A near-year match is returned with
///      `year_tolerance_warning` set so the caller can surface it visibly.
pub fn resolve_films(
    client: &dyn TraktHttpClient,
    base_url: &str,
    access_token: &str,
    films: &[(String, u32)],
) -> Result<(Vec<ResolvedFilm>, Vec<UnmatchedFilm>), String> {
    let mut matched = Vec::new();
    let mut exact_misses: Vec<(String, u32)> = Vec::new();

    // Pass 1: exact year match.
    for (title, year) in films {
        match search_movie_by_title_year(client, base_url, access_token, title, *year)? {
            Some(ids) => matched.push(ResolvedFilm {
                title: title.clone(),
                year: *year,
                ids,
                year_tolerance_warning: None,
            }),
            None => exact_misses.push((title.clone(), *year)),
        }
    }

    // Pass 2: ±1 year tolerance, only for exact-miss films.
    let mut unmatched = Vec::new();
    for (title, year) in &exact_misses {
        match try_adjacent_year_match(client, base_url, access_token, title, *year)? {
            Some((ids, matched_year)) => {
                let warning = format!(
                    "'{}' (year {}) matched Trakt year {} — verify this is the same film",
                    title, year, matched_year
                );
                matched.push(ResolvedFilm {
                    title: title.clone(),
                    year: *year,
                    ids,
                    year_tolerance_warning: Some(warning),
                });
            }
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
        // Second film: no results in first pass or adjacent-year passes.
        let client = MockClient::new(vec![
            (200, &matrix_json), // pass 1: The Matrix → match
            (200, "[]"),         // pass 1: Ghost Film XYZ → no match
            (200, "[]"),         // pass 2: Ghost Film XYZ year-1 (2049) → no match
            (200, "[]"),         // pass 2: Ghost Film XYZ year+1 (2051) → no match
        ]);
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
            (200, &matrix_json),    // pass 1: The Matrix → match
            (200, "[]"),            // pass 1: Ghost Film XYZ → no match
            (200, &inception_json), // pass 1: Inception → match
            (200, "[]"),            // pass 2: Ghost Film XYZ year-1 (2049) → no match
            (200, "[]"),            // pass 2: Ghost Film XYZ year+1 (2051) → no match
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
        // Makes an API call with an empty query param; no adjacent-year hits either.
        let client = MockClient::new(vec![(200, "[]"), (200, "[]"), (200, "[]")]);
        let films = vec![("".to_string(), 2000u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert!(matched.is_empty());
        assert_eq!(unmatched.len(), 1);
    }

    #[test]
    fn whitespace_only_title_goes_to_unmatched_without_panic() {
        // Spaces are encoded (%20) but yield no exact title match or adjacent-year hit.
        let client = MockClient::new(vec![(200, "[]"), (200, "[]"), (200, "[]")]);
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

    // ── FG-15: year-tolerance matching ────────────────────────────────────────

    #[test]
    fn resolve_films_year_minus_one_falls_back_to_near_match() {
        // LB says 2018; Trakt has it under 2017 (festival premiere year).
        // Pass 1 (exact, years=2018) returns [].
        // Pass 2 (year-1, years=2017) returns the film → near-year match.
        let hit_json = one_result("Some Film", 2017, 1, 999, "tt0000001");
        let client = MockClient::new(vec![
            (200, "[]"),      // pass 1: years=2018 → no match
            (200, &hit_json), // pass 2: years=2017 → match
        ]);
        let films = vec![("Some Film".to_string(), 2018u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(matched.len(), 1, "near-year film must be matched");
        assert!(
            unmatched.is_empty(),
            "matched film must not appear in unmatched"
        );
        assert_eq!(matched[0].ids.tmdb_id, Some(999));
        assert!(
            matched[0].year_tolerance_warning.is_some(),
            "near-year match must carry a warning"
        );
        let warn = matched[0].year_tolerance_warning.as_deref().unwrap();
        assert!(
            warn.contains("2017") && warn.contains("2018"),
            "warning must mention both years: {warn}"
        );
    }

    #[test]
    fn resolve_films_year_plus_one_falls_back_to_near_match() {
        // LB says 2017; Trakt has it under 2018 (wide-release year).
        // Pass 1 (exact, years=2017) returns [].
        // Pass 2 (year-1, years=2016) returns [] — nothing there.
        // Pass 2 (year+1, years=2018) returns the film → near-year match.
        let hit_json = one_result("Some Film", 2018, 2, 888, "tt0000002");
        let client = MockClient::new(vec![
            (200, "[]"),      // pass 1: years=2017 → no match
            (200, "[]"),      // pass 2: years=2016 → no match
            (200, &hit_json), // pass 2: years=2018 → match
        ]);
        let films = vec![("Some Film".to_string(), 2017u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(matched.len(), 1, "near-year film must be matched");
        assert!(unmatched.is_empty());
        assert_eq!(matched[0].ids.tmdb_id, Some(888));
        assert!(matched[0].year_tolerance_warning.is_some());
        let warn = matched[0].year_tolerance_warning.as_deref().unwrap();
        assert!(
            warn.contains("2018") && warn.contains("2017"),
            "warning must mention both years: {warn}"
        );
    }

    #[test]
    fn resolve_films_year_tolerance_different_title_no_false_match() {
        // "Original Film" (2018): exact pass fails. Adjacent-year passes return
        // "Sequel Film" at year 2017 and 2019 — different title, must NOT match.
        let sequel_2017 = one_result("Sequel Film", 2017, 10, 100, "tt0000010");
        let sequel_2019 = one_result("Sequel Film", 2019, 11, 101, "tt0000011");
        let client = MockClient::new(vec![
            (200, "[]"),         // pass 1: years=2018 → no match
            (200, &sequel_2017), // pass 2: years=2017 → wrong title, no match
            (200, &sequel_2019), // pass 2: years=2019 → wrong title, no match
        ]);
        let films = vec![("Original Film".to_string(), 2018u32)];
        let (matched, unmatched) =
            resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert!(
            matched.is_empty(),
            "different-title adjacent-year must NOT match"
        );
        assert_eq!(unmatched.len(), 1, "must land in unmatched");
        assert_eq!(unmatched[0].title, "Original Film");
    }

    #[test]
    fn resolve_films_exact_match_has_no_warning() {
        // An exact-year match must never carry a year_tolerance_warning.
        let json = one_result("The Matrix", 1999, 481, 603, "tt0133093");
        let client = MockClient::new(vec![(200, &json)]);
        let films = vec![("The Matrix".to_string(), 1999u32)];
        let (matched, _) = resolve_films(&client, "https://api.trakt.tv", "token", &films).unwrap();
        assert_eq!(matched.len(), 1);
        assert!(
            matched[0].year_tolerance_warning.is_none(),
            "exact-year match must not carry a warning"
        );
    }
}
