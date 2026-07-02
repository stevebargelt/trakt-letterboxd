---
id: FG-4
type: story
status: active
title: "US-004: Trakt write client with rate-limit handling"
created: 2026-07-02
---

**Description:** As the developer, I need to write history, ratings, and watchlist to Trakt so Letterboxd data can be imported.

**Depends on:** US-002

**Acceptance Criteria:**
- [ ] POST /sync/history, /sync/ratings, /sync/watchlist supported
- [ ] Respects write rate limit (1 POST/sec) and honors Retry-After on 429
- [ ] Watched entries submit backdated watched_at ISO-8601 timestamps
- [ ] Ratings submitted on Trakt 1-10 scale
- [ ] cargo test passes (HTTP layer mocked)