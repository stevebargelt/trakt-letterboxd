---
id: FG-10
type: story
status: done
title: "US-010: sync to-letterboxd command (T to L)"
created: 2026-07-02
closed: 2026-07-02
closed_commit: 9d32b037f9727206503aabf81f20b26731d61a53
---

**Description:** As a user, I want to generate a Letterboxd-import CSV from my Trakt data so I can upload it.

**Depends on:** US-003, US-006, US-008

**Acceptance Criteria:**
- [ ] Reads Trakt history/ratings/watchlist (US-003), generates import CSV(s) (US-006), skipping already-exported items (US-008)
- [ ] Writes CSV(s) to the configured data directory and prints their paths
- [ ] Prints clear next-step instructions for the manual Letterboxd upload (which page, which file)
- [ ] --dry-run reports counts without writing files
- [ ] cargo test passes against fixtures with the Trakt client mocked