---
id: FG-7
type: story
status: done
title: "US-007: Cross-platform film matching and rating conversion"
created: 2026-07-02
closed: 2026-07-02
closed_commit: e880cb62c1b9aaf7f8dd9f3b32c43f0caf679d4b
---

**Description:** As the developer, I need to resolve a film from one platform to the other reliably so entries land on the correct title.

**Depends on:** US-003, US-005

**Acceptance Criteria:**
- [ ] Given a Letterboxd export row (no TMDB/IMDb ID), resolve to a Trakt film via title+year search, confirmed against TMDB ID where possible
- [ ] Prefer TMDB ID, then IMDb ID, then title+year as the match strategy
- [ ] Rating conversion helpers cover both directions (x2 and /2) with half-star handling
- [ ] Unmatched films are collected and reported, not silently dropped
- [ ] cargo test passes covering a clean match, a fuzzy match, and an unmatched item