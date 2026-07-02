---
id: FG-11
type: story
status: done
title: "US-011: Best-effort diary date and review handling"
created: 2026-07-02
closed: 2026-07-02
closed_commit: 9fcf959596202084a359e15f3d4a26825b592b17
---

**Description:** As a user, I want diary watch-dates and review text carried across where the platforms allow it, with honest limits.

**Depends on:** US-009, US-010

**Acceptance Criteria:**
- [ ] L->T: Letterboxd diary watched-dates populate Trakt history watched_at; review text attached to Trakt notes where a slot is available, else recorded as "not transferable" in the summary
- [ ] T->L: review text from Trakt notes (if any) written into the CSV Review column
- [ ] Run summary explicitly states how many reviews transferred vs skipped and why
- [ ] cargo test passes covering a review that transfers and one reported as skipped