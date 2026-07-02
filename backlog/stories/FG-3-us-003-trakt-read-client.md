---
id: FG-3
type: story
status: active
title: "US-003: Trakt read client"
created: 2026-07-02
---

**Description:** As the developer, I need to read the user's Trakt data so it can be exported toward Letterboxd.

**Depends on:** US-002

**Acceptance Criteria:**
- [ ] Fetch watched history (with watched_at dates), ratings, and watchlist
- [ ] Handles pagination across large histories
- [ ] Respects GET rate limit (1,000 / 5-min window) with backoff on HTTP 429
- [ ] Each returned film includes its TMDB and IMDb IDs where available
- [ ] cargo test passes (HTTP layer mocked)