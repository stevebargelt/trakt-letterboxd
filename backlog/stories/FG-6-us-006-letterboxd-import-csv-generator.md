---
id: FG-6
type: story
status: active
title: "US-006: Letterboxd import CSV generator"
created: 2026-07-02
---

**Description:** As the developer, I need to produce a Letterboxd-compatible import CSV from Trakt data so the user can upload it.

**Depends on:** US-001

**Acceptance Criteria:**
- [ ] Emits columns: Title, Year, tmdbID, WatchedDate (YYYY-MM-DD), Rating (0.5-5), Rewatch, Tags, Review
- [ ] tmdbID populated from Trakt data for exact matching; rating divided by 2 into half-star scale
- [ ] Output is valid UTF-8 CSV with correct quoting (commas in review text handled)
- [ ] Watchlist emitted as a separate file/section from diary/watched entries
- [ ] cargo test passes verifying header + a representative row matches Letterboxd import spec