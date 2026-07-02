---
id: FG-5
type: story
status: active
title: "US-005: Letterboxd export CSV parser"
created: 2026-07-02
---

**Description:** As the developer, I need to read a Letterboxd data-export ZIP so its contents can be pushed to Trakt.

**Depends on:** US-001

**Acceptance Criteria:**
- [ ] Parses diary.csv, ratings.csv, watchlist.csv, reviews.csv from the export
- [ ] Extracts title, year, Letterboxd URI, rating (0.5-5), watched date, rewatch flag, tags, review text
- [ ] Handles UTF-8, quoted fields containing commas, and missing optional columns
- [ ] Malformed rows are skipped with a warning, not a crash
- [ ] cargo test passes against sample export fixtures