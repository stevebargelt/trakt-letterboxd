#![allow(dead_code)]

pub use crate::rating::trakt_rating_to_letterboxd;
use crate::trakt_read::{RatedMovie, WatchedMovie, WatchlistMovie};
use csv::Writer;
use std::collections::HashMap;
use std::io;

fn truncate_to_date(ts: &str) -> &str {
    ts.split('T').next().unwrap_or(ts)
}

/// Write the Letterboxd diary import CSV.
///
/// `entries` is a slice of `(movie, date_override)` pairs where:
///   - `None`      → use the movie's `watched_at` timestamp truncated to a date
///   - `Some("")`  → emit a blank WatchedDate (mark watched, no diary date)
///   - `Some(s)`   → emit `s` as the literal WatchedDate
///
/// When `include_ratings` is `false` the Rating column is always empty.
pub fn write_diary_csv<W: io::Write>(
    w: W,
    entries: &[(&WatchedMovie, Option<&str>)],
    ratings: &[RatedMovie],
    notes: &HashMap<u64, String>,
    include_ratings: bool,
) -> Result<(), String> {
    let rating_map: HashMap<u64, u8> = ratings
        .iter()
        .filter_map(|r| r.movie.tmdb_id.map(|id| (id, r.rating)))
        .collect();

    let mut wtr = Writer::from_writer(w);
    wtr.write_record([
        "Title",
        "Year",
        "tmdbID",
        "WatchedDate",
        "Rating",
        "Rewatch",
        "Tags",
        "Review",
    ])
    .map_err(|e| e.to_string())?;

    for (entry, date_override) in entries {
        let movie = &entry.movie;
        let year = movie.year.map(|y| y.to_string()).unwrap_or_default();
        let tmdb_id = movie.tmdb_id.map(|id| id.to_string()).unwrap_or_default();
        let watched_date: &str = match date_override {
            None => truncate_to_date(&entry.watched_at),
            Some(s) => s,
        };
        let rating = if include_ratings {
            movie
                .tmdb_id
                .and_then(|id| rating_map.get(&id))
                .map(|&r| format!("{:.1}", trakt_rating_to_letterboxd(r)))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let review = movie
            .tmdb_id
            .and_then(|id| notes.get(&id))
            .map(String::as_str)
            .unwrap_or("");

        let row = [
            movie.title.as_str(),
            year.as_str(),
            tmdb_id.as_str(),
            watched_date,
            rating.as_str(),
            "No",
            "",
            review,
        ];
        wtr.write_record(row).map_err(|e| e.to_string())?;
    }

    wtr.flush().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn write_watchlist_csv<W: io::Write>(w: W, watchlist: &[WatchlistMovie]) -> Result<(), String> {
    let mut wtr = Writer::from_writer(w);
    wtr.write_record(["Title", "Year", "tmdbID"])
        .map_err(|e| e.to_string())?;

    for entry in watchlist {
        let movie = &entry.movie;
        let year = movie.year.map(|y| y.to_string()).unwrap_or_default();
        let tmdb_id = movie.tmdb_id.map(|id| id.to_string()).unwrap_or_default();

        let row = [movie.title.as_str(), year.as_str(), tmdb_id.as_str()];
        wtr.write_record(row).map_err(|e| e.to_string())?;
    }

    wtr.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trakt_read::{MovieRecord, RatedMovie, WatchedMovie, WatchlistMovie};
    use std::collections::HashMap;

    fn make_movie(title: &str, year: u32, tmdb_id: u64) -> MovieRecord {
        MovieRecord {
            title: title.to_string(),
            year: Some(year),
            trakt_id: None,
            slug: None,
            imdb_id: None,
            tmdb_id: Some(tmdb_id),
        }
    }

    fn make_watched(title: &str, year: u32, tmdb_id: u64, watched_at: &str) -> WatchedMovie {
        WatchedMovie {
            watched_at: watched_at.to_string(),
            movie: make_movie(title, year, tmdb_id),
        }
    }

    fn make_rated(tmdb_id: u64, rating: u8) -> RatedMovie {
        RatedMovie {
            rated_at: "2024-01-01T00:00:00.000Z".to_string(),
            rating,
            movie: make_movie("placeholder", 2024, tmdb_id),
        }
    }

    fn make_watchlist_item(title: &str, year: u32, tmdb_id: u64) -> WatchlistMovie {
        WatchlistMovie {
            listed_at: "2024-01-01T00:00:00.000Z".to_string(),
            movie: make_movie(title, year, tmdb_id),
        }
    }

    fn to_lines(out: Vec<u8>) -> Vec<String> {
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| l.to_string())
            .collect()
    }

    /// Build a plain `(movie, None)` entry slice — preserves pre-FG-17 behaviour.
    fn as_entries(history: &[WatchedMovie]) -> Vec<(&WatchedMovie, Option<&str>)> {
        history.iter().map(|m| (m, None)).collect()
    }

    #[test]
    fn diary_header_and_watched_rated_film() {
        let history = vec![make_watched(
            "The Matrix",
            1999,
            603,
            "2024-01-15T20:30:00.000Z",
        )];
        let ratings = vec![make_rated(603, 8)];

        let mut out = Vec::new();
        write_diary_csv(
            &mut out,
            &as_entries(&history),
            &ratings,
            &HashMap::new(),
            true,
        )
        .unwrap();
        let lines = to_lines(out);

        assert_eq!(
            lines[0],
            "Title,Year,tmdbID,WatchedDate,Rating,Rewatch,Tags,Review"
        );
        assert_eq!(lines[1], "The Matrix,1999,603,2024-01-15,4.0,No,,");
    }

    #[test]
    fn diary_date_truncated_to_date_only() {
        let history = vec![make_watched(
            "Inception",
            2010,
            27205,
            "2023-11-22T14:45:30.000Z",
        )];

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();
        let lines = to_lines(out);

        assert!(
            lines[1].contains("2023-11-22"),
            "expected YYYY-MM-DD date in: {}",
            lines[1]
        );
        assert!(
            !lines[1].contains("14:45"),
            "time part must not appear: {}",
            lines[1]
        );
    }

    #[test]
    fn diary_title_with_comma_is_quoted() {
        let history = vec![make_watched(
            "Knives Out, The Sequel",
            2022,
            12345,
            "2024-02-01T00:00:00.000Z",
        )];

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();
        let csv_str = String::from_utf8(out).unwrap();

        assert!(
            csv_str.contains("\"Knives Out, The Sequel\""),
            "title with comma must be quoted:\n{csv_str}"
        );
    }

    #[test]
    fn rating_conversion_values() {
        assert_eq!(trakt_rating_to_letterboxd(1), 0.5);
        assert_eq!(trakt_rating_to_letterboxd(8), 4.0);
        assert_eq!(trakt_rating_to_letterboxd(9), 4.5);
        assert_eq!(trakt_rating_to_letterboxd(10), 5.0);
    }

    #[test]
    fn diary_half_star_rating_in_csv() {
        let history = vec![make_watched(
            "Dune",
            2021,
            438631,
            "2024-03-01T00:00:00.000Z",
        )];
        let ratings = vec![make_rated(438631, 9)];

        let mut out = Vec::new();
        write_diary_csv(
            &mut out,
            &as_entries(&history),
            &ratings,
            &HashMap::new(),
            true,
        )
        .unwrap();
        let lines = to_lines(out);

        assert_eq!(lines[1], "Dune,2021,438631,2024-03-01,4.5,No,,");
    }

    #[test]
    fn diary_unrated_film_empty_rating_field() {
        let history = vec![make_watched(
            "Blade Runner",
            1982,
            78,
            "2024-04-01T00:00:00.000Z",
        )];

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();
        let lines = to_lines(out);

        assert_eq!(lines[1], "Blade Runner,1982,78,2024-04-01,,No,,");
    }

    #[test]
    fn watchlist_header_and_rows() {
        let watchlist = vec![
            make_watchlist_item("Oppenheimer", 2023, 872585),
            make_watchlist_item("Past Lives", 2023, 1015777),
        ];

        let mut out = Vec::new();
        write_watchlist_csv(&mut out, &watchlist).unwrap();
        let lines = to_lines(out);

        assert_eq!(lines[0], "Title,Year,tmdbID");
        assert_eq!(lines[1], "Oppenheimer,2023,872585");
        assert_eq!(lines[2], "Past Lives,2023,1015777");
    }

    // GAP 1: Rating conversion boundaries — table-driven, includes odd value 7->3.5.
    #[test]
    fn rating_conversion_boundaries_table() {
        let cases: &[(u8, f32)] = &[(1, 0.5), (7, 3.5), (10, 5.0)];
        for &(trakt, expected) in cases {
            assert_eq!(
                trakt_rating_to_letterboxd(trakt),
                expected,
                "trakt {trakt} should map to {expected}"
            );
        }
    }

    // GAP 2: Rewatch column — current code always emits "No" (no rewatch field in WatchedMovie).
    // NOTE: the data model has no rewatch indicator; Letterboxd import will treat every
    // entry as a first watch. A future story should add plays/rewatch support to WatchedMovie.
    #[test]
    fn diary_rewatch_column_is_always_no() {
        let history = vec![make_watched(
            "Mad Max: Fury Road",
            2015,
            76341,
            "2024-05-01T00:00:00.000Z",
        )];
        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();
        let lines = to_lines(out);
        // Rewatch is the 6th field (index 5). Assert the literal substring ",No," appears.
        assert!(
            lines[1].contains(",No,"),
            "Rewatch column must be 'No' (hardcoded — no rewatch field in model): {}",
            lines[1]
        );
    }

    // GAP 3: Title containing a newline and a double-quote round-trips correctly through
    // the csv crate's quoting.  Review is always empty in the current implementation, so
    // we verify quoting behaviour via the Title column (same code path for any string field).
    #[test]
    fn diary_title_with_newline_and_quote_round_trips() {
        let tricky_title = "Say \"Hello\"\nWorld";
        let history = vec![make_watched(
            tricky_title,
            2024,
            99999,
            "2024-06-01T00:00:00.000Z",
        )];
        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();

        // Parse back with csv::Reader; it must reconstruct the original string exactly.
        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr
            .records()
            .next()
            .expect("expected one data row")
            .expect("csv parse error");
        assert_eq!(
            &record[0], tricky_title,
            "title with newline and double-quote must round-trip cleanly"
        );
    }

    // GAP 4: A watched film with no tmdb_id must still emit a row (not be dropped).
    // Letterboxd can match on Title+Year even without a tmdbID.
    #[test]
    fn diary_watched_film_without_tmdb_id_emits_row() {
        let movie = MovieRecord {
            title: "Obscure Film".to_string(),
            year: Some(1995),
            trakt_id: Some(99),
            slug: None,
            imdb_id: None,
            tmdb_id: None,
        };
        let history = vec![WatchedMovie {
            watched_at: "2024-07-01T00:00:00.000Z".to_string(),
            movie,
        }];
        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &HashMap::new(), true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr
            .records()
            .next()
            .expect("film without tmdb_id must still emit a row — dropping it loses data")
            .expect("csv parse error");
        assert_eq!(&record[0], "Obscure Film", "Title must be present");
        assert_eq!(&record[1], "1995", "Year must be present");
        assert_eq!(
            &record[2], "",
            "tmdbID column must be empty when tmdb_id is None"
        );
    }

    // GAP 5: Empty history and ratings produces a header-only CSV (not a crash or empty output).
    #[test]
    fn diary_empty_input_produces_header_only() {
        let mut out = Vec::new();
        write_diary_csv(&mut out, &[], &[], &HashMap::new(), true).unwrap();
        let lines = to_lines(out);
        assert_eq!(
            lines.len(),
            1,
            "empty input should produce only the header row, got: {:?}",
            lines
        );
        assert_eq!(
            lines[0],
            "Title,Year,tmdbID,WatchedDate,Rating,Rewatch,Tags,Review"
        );
    }

    // GAP 6: A rated film that was NOT watched does NOT appear in the diary CSV.
    // The diary tracks watch events; ratings without a watch entry are intentionally excluded.
    // This matches Letterboxd's diary model (import by watched date).
    #[test]
    fn diary_rated_but_not_watched_excluded() {
        let ratings = vec![make_rated(12345, 8)];
        let mut out = Vec::new();
        write_diary_csv(&mut out, &[], &ratings, &HashMap::new(), true).unwrap();
        let lines = to_lines(out);
        assert_eq!(
            lines.len(),
            1,
            "a film rated but never watched must not appear in the diary CSV"
        );
    }

    #[test]
    fn diary_note_populates_review_column() {
        let history = vec![make_watched(
            "The Matrix",
            1999,
            603,
            "2024-01-15T20:30:00.000Z",
        )];
        let mut notes = HashMap::new();
        notes.insert(603u64, "An absolute masterpiece.".to_string());

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &notes, true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[7], "An absolute masterpiece.",
            "Review column must contain note text"
        );
    }

    #[test]
    fn diary_review_with_commas_and_newline_round_trips() {
        // A review that contains both a comma and an embedded newline must survive
        // the csv crate's quoting and parse back to the exact original string.
        let tricky_review = "Great film, loved it.\nWould watch again.";
        let history = vec![make_watched(
            "The Matrix",
            1999,
            603,
            "2024-01-15T20:30:00.000Z",
        )];
        let mut notes = HashMap::new();
        notes.insert(603u64, tricky_review.to_string());

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &notes, true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr
            .records()
            .next()
            .expect("expected one data row")
            .expect("csv parse error");
        assert_eq!(
            &record[7], tricky_review,
            "review with commas and newline must round-trip cleanly through CSV"
        );
    }

    #[test]
    fn diary_film_without_note_has_empty_review_column() {
        let history = vec![make_watched(
            "Inception",
            2010,
            27205,
            "2024-01-15T20:30:00.000Z",
        )];
        let mut notes = HashMap::new();
        notes.insert(603u64, "Note for a different film".to_string());

        let mut out = Vec::new();
        write_diary_csv(&mut out, &as_entries(&history), &[], &notes, true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[7], "",
            "Review column must be empty when no note for this film"
        );
    }

    // --- FG-17 new tests ---

    #[test]
    fn diary_some_empty_string_date_override_emits_blank_watched_date() {
        let movie = make_watched("Dune", 2021, 438631, "2023-09-10T00:00:00.000Z");
        // Some("") means bulk-date net-new: mark watched, no diary date
        let entries: Vec<(&WatchedMovie, Option<&str>)> = vec![(&movie, Some(""))];

        let mut out = Vec::new();
        write_diary_csv(&mut out, &entries, &[], &HashMap::new(), true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[3], "",
            "WatchedDate must be blank when date_override is Some(\"\")"
        );
    }

    #[test]
    fn diary_some_date_override_emits_literal_date() {
        let movie = make_watched("Inception", 2010, 27205, "2023-09-10T00:00:00.000Z");
        let entries: Vec<(&WatchedMovie, Option<&str>)> = vec![(&movie, Some("2023-09-10"))];

        let mut out = Vec::new();
        write_diary_csv(&mut out, &entries, &[], &HashMap::new(), true).unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[3], "2023-09-10",
            "WatchedDate must be the literal override string"
        );
    }

    #[test]
    fn diary_include_ratings_false_suppresses_rating_column() {
        let history = vec![make_watched(
            "The Matrix",
            1999,
            603,
            "2024-01-15T20:30:00.000Z",
        )];
        let ratings = vec![make_rated(603, 8)];

        let mut out = Vec::new();
        write_diary_csv(
            &mut out,
            &as_entries(&history),
            &ratings,
            &HashMap::new(),
            false, // include_ratings = false
        )
        .unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[4], "",
            "Rating column must be empty when include_ratings=false, got: {:?}",
            &record[4]
        );
    }

    #[test]
    fn diary_include_ratings_true_preserves_rating_conversion() {
        let history = vec![make_watched(
            "The Matrix",
            1999,
            603,
            "2024-01-15T20:30:00.000Z",
        )];
        let ratings = vec![make_rated(603, 8)];

        let mut out = Vec::new();
        write_diary_csv(
            &mut out,
            &as_entries(&history),
            &ratings,
            &HashMap::new(),
            true, // include_ratings = true
        )
        .unwrap();

        let mut rdr = csv::Reader::from_reader(out.as_slice());
        let record = rdr.records().next().unwrap().unwrap();
        assert_eq!(
            &record[4], "4.0",
            "Rating column must contain converted rating when include_ratings=true"
        );
    }
}
