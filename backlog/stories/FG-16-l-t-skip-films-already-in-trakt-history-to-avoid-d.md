---
id: FG-16
type: story
status: active
title: "L->T: skip films already in Trakt history to avoid duplicate plays"
created: 2026-07-02
---

**Description:** As a user, when I sync Letterboxd -> Trakt I do NOT want films already in my Trakt history to get a second "play" entry dated to a Letterboxd logging date, so my Trakt history is not polluted with phantom rewatches.

**Discovered during:** the FG e2e live test (2026-07-02). Root issues:
1. `watched.csv` from Letterboxd has NO real watch date (only a logged Date, often a bulk-import day). The tool backdates Trakt watched_at to that logged date.
2. Trakt POST /sync/history ADDS a play each call; it does not dedupe against the user account's EXISTING history. Our sync_state only knows about our own prior syncs (empty on first run). So a first sync of overlapping data creates a duplicate play per already-watched film, all dated to the logging day.

**Approach options:**
- Before adding history, fetch the user's existing Trakt watched history (fetch_watched_history, already available) and SKIP films already present (by tmdb id), OR skip only when a play already exists for that film.
- And/or a `--no-dates` mode, and/or treat watched.csv (dateless) films differently from diary.csv (real-dated) films.
- Report skipped-as-already-on-trakt distinctly from skipped-as-already-synced.

**Acceptance Criteria:**
- [ ] L->T does not create a duplicate play for a film already in the user's Trakt history
- [ ] diary.csv real watch dates still sync correctly for genuinely-new films
- [ ] Summary distinguishes already-on-Trakt from newly-added
- [ ] Tests cover: film already on Trakt -> skipped; new film -> added; dateless watched.csv handling
- [ ] cargo test / clippy / fmt clean