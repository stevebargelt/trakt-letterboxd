---
id: FG-9
type: story
status: done
title: "US-009: sync from-letterboxd command (L to T)"
created: 2026-07-02
closed: 2026-07-02
closed_commit: 40aee8011f582b5b713f7cd293db7123ad0b3b7a
---

**Description:** As a user, I want to push my exported Letterboxd data into Trakt in one command.

**Depends on:** US-004, US-007, US-008

**Acceptance Criteria:**
- [ ] Command takes a path to the Letterboxd export ZIP (or extracted folder)
- [ ] Parses (US-005) -> matches (US-007) -> writes history/ratings/watchlist to Trakt (US-004), skipping already-synced (US-008)
- [ ] Prints a summary: N watched, N ratings, N watchlist added; M unmatched (listed)
- [ ] --dry-run reports the same summary without writing to Trakt
- [ ] cargo test passes against fixtures with the Trakt client mocked