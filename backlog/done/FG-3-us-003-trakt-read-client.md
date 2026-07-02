---
id: FG-3
type: story
status: done
title: "US-003: Trakt read client"
created: 2026-07-02
closed: 2026-07-02
closed_commit: 2afe59e0cc09562396277c7b747c88c9dac723c8
---

**Description:** As the developer, I need to read the user's Trakt data so it can be exported toward Letterboxd.

**Depends on:** US-002

**Acceptance Criteria:**
- [ ] Fetch watched history (with watched_at dates), ratings, and watchlist
- [ ] Handles pagination across large histories
- [ ] Respects GET rate limit (1,000 / 5-min window) with backoff on HTTP 429
- [ ] Each returned film includes its TMDB and IMDb IDs where available
- [ ] cargo test passes (HTTP layer mocked)