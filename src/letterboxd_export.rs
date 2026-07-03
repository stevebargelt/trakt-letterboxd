use csv::ReaderBuilder;
use std::fs;
use std::io::{Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

#[derive(Debug, Clone)]
pub struct DiaryEntry {
    pub logged_date: String,
    pub name: String,
    pub year: u32,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub letterboxd_uri: String,
    pub slug: String,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub rating: Option<f32>,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub rewatch: bool,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub tags: Vec<String>,
    pub watched_date: String,
}

#[derive(Debug, Clone)]
pub struct WatchedEntry {
    pub logged_date: String,
    pub name: String,
    pub year: u32,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub letterboxd_uri: String,
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct RatingEntry {
    pub logged_date: String,
    pub name: String,
    pub year: u32,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub letterboxd_uri: String,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub slug: String,
    pub rating: f32,
}

#[derive(Debug, Clone)]
pub struct WatchlistEntry {
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub logged_date: String,
    pub name: String,
    pub year: u32,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub letterboxd_uri: String,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub slug: String,
}

#[derive(Debug, Clone)]
pub struct ReviewEntry {
    pub logged_date: String,
    pub name: String,
    pub year: u32,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub letterboxd_uri: String,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub slug: String,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub rating: Option<f32>,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub rewatch: bool,
    #[allow(dead_code)] // parsed from CSV; not consumed by sync logic
    pub tags: Vec<String>,
    pub watched_date: String,
    pub review: String,
}

#[derive(Debug, Default)]
pub struct LetterboxdExport {
    pub diary: Vec<DiaryEntry>,
    pub watched: Vec<WatchedEntry>,
    pub ratings: Vec<RatingEntry>,
    pub watchlist: Vec<WatchlistEntry>,
    pub reviews: Vec<ReviewEntry>,
}

impl LetterboxdExport {
    pub fn load(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            load_from_dir(path)
        } else {
            load_from_zip(path)
        }
    }
}

fn slug_from_uri(uri: &str) -> String {
    // Strip query string / fragment before extracting the last path segment.
    let path = uri.split('?').next().unwrap_or(uri);
    let path = path.split('#').next().unwrap_or(path);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

fn col_idx(headers: &csv::StringRecord, name: &str) -> Option<usize> {
    headers.iter().position(|h| h == name)
}

fn get_field(record: &csv::StringRecord, idx: Option<usize>) -> &str {
    idx.and_then(|i| record.get(i)).unwrap_or("")
}

fn tags_from_field(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![];
    }
    s.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn parse_diary(content: &str) -> Vec<DiaryEntry> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_reader(Cursor::new(content));
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("warning: diary.csv: cannot read headers: {e}");
            return vec![];
        }
    };

    let ci_date = col_idx(&headers, "Date");
    let ci_name = col_idx(&headers, "Name");
    let ci_year = col_idx(&headers, "Year");
    let ci_uri = col_idx(&headers, "Letterboxd URI");
    let ci_rating = col_idx(&headers, "Rating");
    let ci_rewatch = col_idx(&headers, "Rewatch");
    let ci_tags = col_idx(&headers, "Tags");
    let ci_watched = col_idx(&headers, "Watched Date");

    let mut entries = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let row = i + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: diary.csv row {row}: {e}, skipping");
                continue;
            }
        };

        let name = get_field(&record, ci_name);
        let year_str = get_field(&record, ci_year);
        let uri = get_field(&record, ci_uri);

        if name.is_empty() || year_str.is_empty() || uri.is_empty() {
            eprintln!("warning: diary.csv row {row}: missing required field, skipping");
            continue;
        }

        let year = match year_str.parse::<u32>() {
            Ok(y) => y,
            Err(_) => {
                eprintln!("warning: diary.csv row {row}: invalid year '{year_str}', skipping");
                continue;
            }
        };

        let rating_str = get_field(&record, ci_rating);
        let rating = if rating_str.is_empty() {
            None
        } else {
            match rating_str.parse::<f32>() {
                Ok(r) => Some(r),
                Err(_) => {
                    eprintln!(
                        "warning: diary.csv row {row}: invalid rating '{rating_str}', skipping"
                    );
                    continue;
                }
            }
        };

        entries.push(DiaryEntry {
            logged_date: get_field(&record, ci_date).to_string(),
            name: name.to_string(),
            year,
            letterboxd_uri: uri.to_string(),
            slug: slug_from_uri(uri),
            rating,
            rewatch: get_field(&record, ci_rewatch).eq_ignore_ascii_case("yes"),
            tags: tags_from_field(get_field(&record, ci_tags)),
            watched_date: get_field(&record, ci_watched).to_string(),
        });
    }
    entries
}

fn parse_watched(content: &str) -> Vec<WatchedEntry> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_reader(Cursor::new(content));
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("warning: watched.csv: cannot read headers: {e}");
            return vec![];
        }
    };

    let ci_date = col_idx(&headers, "Date");
    let ci_name = col_idx(&headers, "Name");
    let ci_year = col_idx(&headers, "Year");
    let ci_uri = col_idx(&headers, "Letterboxd URI");

    let mut entries = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let row = i + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: watched.csv row {row}: {e}, skipping");
                continue;
            }
        };

        let name = get_field(&record, ci_name);
        let year_str = get_field(&record, ci_year);
        let uri = get_field(&record, ci_uri);

        if name.is_empty() || year_str.is_empty() || uri.is_empty() {
            eprintln!("warning: watched.csv row {row}: missing required field, skipping");
            continue;
        }

        let year = match year_str.parse::<u32>() {
            Ok(y) => y,
            Err(_) => {
                eprintln!("warning: watched.csv row {row}: invalid year '{year_str}', skipping");
                continue;
            }
        };

        entries.push(WatchedEntry {
            logged_date: get_field(&record, ci_date).to_string(),
            name: name.to_string(),
            year,
            letterboxd_uri: uri.to_string(),
            slug: slug_from_uri(uri),
        });
    }
    entries
}

fn parse_ratings(content: &str) -> Vec<RatingEntry> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_reader(Cursor::new(content));
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("warning: ratings.csv: cannot read headers: {e}");
            return vec![];
        }
    };

    let ci_date = col_idx(&headers, "Date");
    let ci_name = col_idx(&headers, "Name");
    let ci_year = col_idx(&headers, "Year");
    let ci_uri = col_idx(&headers, "Letterboxd URI");
    let ci_rating = col_idx(&headers, "Rating");

    let mut entries = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let row = i + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: ratings.csv row {row}: {e}, skipping");
                continue;
            }
        };

        let name = get_field(&record, ci_name);
        let year_str = get_field(&record, ci_year);
        let uri = get_field(&record, ci_uri);
        let rating_str = get_field(&record, ci_rating);

        if name.is_empty() || year_str.is_empty() || uri.is_empty() {
            eprintln!("warning: ratings.csv row {row}: missing required field, skipping");
            continue;
        }

        let year = match year_str.parse::<u32>() {
            Ok(y) => y,
            Err(_) => {
                eprintln!("warning: ratings.csv row {row}: invalid year '{year_str}', skipping");
                continue;
            }
        };

        if rating_str.is_empty() {
            eprintln!("warning: ratings.csv row {row}: missing rating, skipping");
            continue;
        }

        let rating = match rating_str.parse::<f32>() {
            Ok(r) => r,
            Err(_) => {
                eprintln!(
                    "warning: ratings.csv row {row}: invalid rating '{rating_str}', skipping"
                );
                continue;
            }
        };

        entries.push(RatingEntry {
            logged_date: get_field(&record, ci_date).to_string(),
            name: name.to_string(),
            year,
            letterboxd_uri: uri.to_string(),
            slug: slug_from_uri(uri),
            rating,
        });
    }
    entries
}

fn parse_watchlist(content: &str) -> Vec<WatchlistEntry> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_reader(Cursor::new(content));
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("warning: watchlist.csv: cannot read headers: {e}");
            return vec![];
        }
    };

    let ci_date = col_idx(&headers, "Date");
    let ci_name = col_idx(&headers, "Name");
    let ci_year = col_idx(&headers, "Year");
    let ci_uri = col_idx(&headers, "Letterboxd URI");

    let mut entries = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let row = i + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: watchlist.csv row {row}: {e}, skipping");
                continue;
            }
        };

        let name = get_field(&record, ci_name);
        let year_str = get_field(&record, ci_year);
        let uri = get_field(&record, ci_uri);

        if name.is_empty() || year_str.is_empty() || uri.is_empty() {
            eprintln!("warning: watchlist.csv row {row}: missing required field, skipping");
            continue;
        }

        let year = match year_str.parse::<u32>() {
            Ok(y) => y,
            Err(_) => {
                eprintln!("warning: watchlist.csv row {row}: invalid year '{year_str}', skipping");
                continue;
            }
        };

        entries.push(WatchlistEntry {
            logged_date: get_field(&record, ci_date).to_string(),
            name: name.to_string(),
            year,
            letterboxd_uri: uri.to_string(),
            slug: slug_from_uri(uri),
        });
    }
    entries
}

fn parse_reviews(content: &str) -> Vec<ReviewEntry> {
    let mut rdr = ReaderBuilder::new()
        .flexible(true)
        .from_reader(Cursor::new(content));
    let headers = match rdr.headers() {
        Ok(h) => h.clone(),
        Err(e) => {
            eprintln!("warning: reviews.csv: cannot read headers: {e}");
            return vec![];
        }
    };

    let ci_date = col_idx(&headers, "Date");
    let ci_name = col_idx(&headers, "Name");
    let ci_year = col_idx(&headers, "Year");
    let ci_uri = col_idx(&headers, "Letterboxd URI");
    let ci_rating = col_idx(&headers, "Rating");
    let ci_rewatch = col_idx(&headers, "Rewatch");
    let ci_tags = col_idx(&headers, "Tags");
    let ci_watched = col_idx(&headers, "Watched Date");
    let ci_review = col_idx(&headers, "Review");

    let mut entries = Vec::new();
    for (i, result) in rdr.records().enumerate() {
        let row = i + 2;
        let record = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("warning: reviews.csv row {row}: {e}, skipping");
                continue;
            }
        };

        let name = get_field(&record, ci_name);
        let year_str = get_field(&record, ci_year);
        let uri = get_field(&record, ci_uri);

        if name.is_empty() || year_str.is_empty() || uri.is_empty() {
            eprintln!("warning: reviews.csv row {row}: missing required field, skipping");
            continue;
        }

        let year = match year_str.parse::<u32>() {
            Ok(y) => y,
            Err(_) => {
                eprintln!("warning: reviews.csv row {row}: invalid year '{year_str}', skipping");
                continue;
            }
        };

        let rating_str = get_field(&record, ci_rating);
        let rating = if rating_str.is_empty() {
            None
        } else {
            match rating_str.parse::<f32>() {
                Ok(r) => Some(r),
                Err(_) => {
                    eprintln!(
                        "warning: reviews.csv row {row}: invalid rating '{rating_str}', skipping"
                    );
                    continue;
                }
            }
        };

        entries.push(ReviewEntry {
            logged_date: get_field(&record, ci_date).to_string(),
            name: name.to_string(),
            year,
            letterboxd_uri: uri.to_string(),
            slug: slug_from_uri(uri),
            rating,
            rewatch: get_field(&record, ci_rewatch).eq_ignore_ascii_case("yes"),
            tags: tags_from_field(get_field(&record, ci_tags)),
            watched_date: get_field(&record, ci_watched).to_string(),
            review: get_field(&record, ci_review).to_string(),
        });
    }
    entries
}

fn read_zip_entry(archive: &mut ZipArchive<fs::File>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut content = String::new();
    entry.read_to_string(&mut content).ok()?;
    Some(content)
}

pub fn load_from_zip(path: &Path) -> Result<LetterboxdExport, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("cannot open '{}': {e}", path.display()))?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| format!("invalid zip '{}': {e}", path.display()))?;

    Ok(LetterboxdExport {
        diary: read_zip_entry(&mut archive, "diary.csv")
            .map(|c| parse_diary(&c))
            .unwrap_or_default(),
        watched: read_zip_entry(&mut archive, "watched.csv")
            .map(|c| parse_watched(&c))
            .unwrap_or_default(),
        ratings: read_zip_entry(&mut archive, "ratings.csv")
            .map(|c| parse_ratings(&c))
            .unwrap_or_default(),
        watchlist: read_zip_entry(&mut archive, "watchlist.csv")
            .map(|c| parse_watchlist(&c))
            .unwrap_or_default(),
        reviews: read_zip_entry(&mut archive, "reviews.csv")
            .map(|c| parse_reviews(&c))
            .unwrap_or_default(),
    })
}

pub fn load_from_dir(path: &Path) -> Result<LetterboxdExport, String> {
    if !path.is_dir() {
        return Err(format!("'{}' is not a directory", path.display()));
    }

    let read_file = |name: &str| -> Option<String> { fs::read_to_string(path.join(name)).ok() };

    Ok(LetterboxdExport {
        diary: read_file("diary.csv")
            .map(|c| parse_diary(&c))
            .unwrap_or_default(),
        watched: read_file("watched.csv")
            .map(|c| parse_watched(&c))
            .unwrap_or_default(),
        ratings: read_file("ratings.csv")
            .map(|c| parse_ratings(&c))
            .unwrap_or_default(),
        watchlist: read_file("watchlist.csv")
            .map(|c| parse_watchlist(&c))
            .unwrap_or_default(),
        reviews: read_file("reviews.csv")
            .map(|c| parse_reviews(&c))
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIARY_CSV: &str = r#"Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date
2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5,No,sci-fi action,1999-03-31
2024-02-01,"Barbie, The Movie",2023,https://letterboxd.com/film/barbie/,,Yes,,2023-07-21
2024-03-01,Bad Data,NOT_A_YEAR,https://letterboxd.com/film/bad-data/,1.0,No,,2024-03-01
"#;

    const WATCHED_CSV: &str = r#"Date,Name,Year,Letterboxd URI
2024-01-15,Oppenheimer,2023,https://letterboxd.com/film/oppenheimer/
"#;

    const RATINGS_CSV: &str = r#"Date,Name,Year,Letterboxd URI,Rating
2024-01-15,The Matrix,1999,https://letterboxd.com/film/the-matrix/,4.5
"#;

    const WATCHLIST_CSV: &str = r#"Date,Name,Year,Letterboxd URI
2024-01-15,Dune,2021,https://letterboxd.com/film/dune-2021/
"#;

    const REVIEWS_CSV: &str = r#"Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review
2024-04-01,Oppenheimer,2023,https://letterboxd.com/film/oppenheimer/,5.0,No,,2023-07-21,"A stunning film, absolutely breathtaking. Nolan at his best."
"#;

    #[test]
    fn test_diary_valid_rows_parsed() {
        let entries = parse_diary(DIARY_CSV);
        assert_eq!(entries.len(), 2, "malformed year row should be skipped");

        let matrix = &entries[0];
        assert_eq!(matrix.name, "The Matrix");
        assert_eq!(matrix.year, 1999);
        assert_eq!(matrix.slug, "the-matrix");
        assert_eq!(matrix.rating, Some(4.5));
        assert!(!matrix.rewatch);
        assert_eq!(matrix.watched_date, "1999-03-31");
        assert_eq!(matrix.tags, vec!["sci-fi action"]);
    }

    #[test]
    fn test_diary_quoted_name_with_comma() {
        let entries = parse_diary(DIARY_CSV);
        let barbie = &entries[1];
        assert_eq!(barbie.name, "Barbie, The Movie");
        assert_eq!(barbie.rating, None);
        assert!(barbie.rewatch);
        assert!(barbie.tags.is_empty());
    }

    #[test]
    fn test_diary_skips_malformed_year() {
        let entries = parse_diary(DIARY_CSV);
        assert!(entries.iter().all(|e| e.name != "Bad Data"));
    }

    #[test]
    fn test_reviews_comma_in_review_text() {
        let entries = parse_reviews(REVIEWS_CSV);
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert!(entry.review.contains(','));
        assert_eq!(entry.slug, "oppenheimer");
        assert_eq!(entry.rating, Some(5.0));
        assert!(!entry.rewatch);
    }

    #[test]
    fn test_watched_parsing() {
        let entries = parse_watched(WATCHED_CSV);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Oppenheimer");
        assert_eq!(entries[0].slug, "oppenheimer");
        assert_eq!(entries[0].year, 2023);
    }

    #[test]
    fn test_ratings_parsing() {
        let entries = parse_ratings(RATINGS_CSV);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rating, 4.5);
        assert_eq!(entries[0].name, "The Matrix");
    }

    #[test]
    fn test_watchlist_parsing() {
        let entries = parse_watchlist(WATCHLIST_CSV);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Dune");
        assert_eq!(entries[0].slug, "dune-2021");
    }

    #[test]
    fn test_slug_extraction() {
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/the-matrix/"),
            "the-matrix"
        );
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/2001-a-space-odyssey/"),
            "2001-a-space-odyssey"
        );
    }

    #[test]
    fn test_missing_files_yield_empty_vecs() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let mut f = fs::File::create(dir.path().join("diary.csv")).unwrap();
        f.write_all(DIARY_CSV.as_bytes()).unwrap();

        let export = load_from_dir(dir.path()).unwrap();
        assert_eq!(export.diary.len(), 2);
        assert!(export.watched.is_empty());
        assert!(export.ratings.is_empty());
        assert!(export.watchlist.is_empty());
        assert!(export.reviews.is_empty());
    }

    #[test]
    fn test_load_from_zip() {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::write::ZipWriter;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        {
            let file = tmp.reopen().unwrap();
            let mut zw = ZipWriter::new(file);
            let opts = SimpleFileOptions::default();
            zw.start_file("diary.csv", opts).unwrap();
            zw.write_all(DIARY_CSV.as_bytes()).unwrap();
            zw.start_file("reviews.csv", opts).unwrap();
            zw.write_all(REVIEWS_CSV.as_bytes()).unwrap();
            zw.finish().unwrap();
        }

        let export = load_from_zip(tmp.path()).unwrap();
        assert_eq!(export.diary.len(), 2);
        assert_eq!(export.reviews.len(), 1);
        assert!(export.watched.is_empty());
    }

    // --- gap-coverage tests ---

    #[test]
    fn test_review_with_multiline_quoted_field() {
        // A Review field that contains an embedded newline inside a quoted value
        // must parse as a single record, not be split into two.
        let csv = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review\n\
2024-04-01,Test Film,2020,https://letterboxd.com/film/test-film/,4.0,No,,2020-01-01,\"First line\nSecond line\"\n";
        let entries = parse_reviews(csv);
        assert_eq!(entries.len(), 1, "multiline review should be one record");
        assert!(
            entries[0].review.contains('\n'),
            "review text should contain embedded newline"
        );
        assert!(entries[0].review.contains("First line"));
        assert!(entries[0].review.contains("Second line"));
    }

    #[test]
    fn test_rewatch_yes_no_and_blank() {
        let csv = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
2024-01-01,Film A,2020,https://letterboxd.com/film/film-a/,,Yes,,2020-01-01\n\
2024-01-02,Film B,2020,https://letterboxd.com/film/film-b/,,No,,2020-01-02\n\
2024-01-03,Film C,2020,https://letterboxd.com/film/film-c/,,,, 2020-01-03\n";
        let entries = parse_diary(csv);
        assert_eq!(entries.len(), 3);
        assert!(entries[0].rewatch, "Yes should map to true");
        assert!(!entries[1].rewatch, "No should map to false");
        assert!(!entries[2].rewatch, "blank Rewatch should map to false");
    }

    #[test]
    fn test_tags_multiple_comma_separated_and_empty() {
        let csv = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
2024-01-01,Film A,2020,https://letterboxd.com/film/film-a/,,No,\"sci-fi, action, thriller\",2020-01-01\n\
2024-01-02,Film B,2020,https://letterboxd.com/film/film-b/,,No,,2020-01-02\n";
        let entries = parse_diary(csv);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].tags,
            vec!["sci-fi", "action", "thriller"],
            "multiple tags should be split and trimmed"
        );
        assert!(
            entries[1].tags.is_empty(),
            "empty Tags field should yield empty vec"
        );
    }

    #[test]
    fn test_rating_half_star_preserved_and_blank_is_none() {
        let diary_csv = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
2024-01-01,Film A,2020,https://letterboxd.com/film/film-a/,3.5,No,,2020-01-01\n\
2024-01-02,Film B,2020,https://letterboxd.com/film/film-b/,,No,,2020-01-02\n";
        let diary = parse_diary(diary_csv);
        assert_eq!(diary.len(), 2);
        assert_eq!(
            diary[0].rating,
            Some(3.5_f32),
            "3.5 half-star rating must be preserved"
        );
        assert_eq!(diary[1].rating, None, "blank diary rating must be None");

        // ratings.csv uses f32 (not Option); a blank rating row is skipped entirely.
        let ratings_csv_half = "Date,Name,Year,Letterboxd URI,Rating\n\
2024-01-01,Film A,2020,https://letterboxd.com/film/film-a/,3.5\n";
        let ratings = parse_ratings(ratings_csv_half);
        assert_eq!(ratings.len(), 1);
        assert_eq!(
            ratings[0].rating, 3.5_f32,
            "3.5 half-star preserved in ratings.csv"
        );

        let ratings_csv_blank = "Date,Name,Year,Letterboxd URI,Rating\n\
2024-01-01,Film A,2020,https://letterboxd.com/film/film-a/,\n";
        let ratings_blank = parse_ratings(ratings_csv_blank);
        assert!(
            ratings_blank.is_empty(),
            "blank rating in ratings.csv skips the row (RatingEntry has no Option)"
        );
    }

    #[test]
    fn test_header_only_file_yields_empty_vec() {
        let diary_only_header = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n";
        assert!(parse_diary(diary_only_header).is_empty());

        let reviews_only_header =
            "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date,Review\n";
        assert!(parse_reviews(reviews_only_header).is_empty());

        let ratings_only_header = "Date,Name,Year,Letterboxd URI,Rating\n";
        assert!(parse_ratings(ratings_only_header).is_empty());

        let watched_only_header = "Date,Name,Year,Letterboxd URI\n";
        assert!(parse_watched(watched_only_header).is_empty());
    }

    #[test]
    fn test_slug_trailing_slash_and_query_segments() {
        // trailing slash already stripped
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/the-matrix/"),
            "the-matrix"
        );
        // no trailing slash
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/the-matrix"),
            "the-matrix"
        );
        // query string must be stripped before extracting slug
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/the-matrix/?ref=profile"),
            "the-matrix"
        );
        // extra path segments before the slug (rsplit takes the rightmost segment)
        assert_eq!(
            slug_from_uri("https://letterboxd.com/username/films/diary/film/the-matrix/"),
            "the-matrix"
        );
        // fragment stripped
        assert_eq!(
            slug_from_uri("https://letterboxd.com/film/the-matrix/#reviews"),
            "the-matrix"
        );
    }

    #[test]
    fn test_utf8_accented_film_title_preserved() {
        let csv = "Date,Name,Year,Letterboxd URI,Rating,Rewatch,Tags,Watched Date\n\
2024-01-01,Amélie,2001,https://letterboxd.com/film/amelie/,5.0,No,,2001-04-25\n\
2024-01-02,Das weiße Band,2009,https://letterboxd.com/film/das-weisse-band/,4.0,No,,2009-01-01\n";
        let entries = parse_diary(csv);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "Amélie");
        assert_eq!(entries[1].name, "Das weiße Band");
    }
}
