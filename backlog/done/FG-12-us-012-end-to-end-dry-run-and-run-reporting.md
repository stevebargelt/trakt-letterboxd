---
id: FG-12
type: story
status: done
title: "US-012: End-to-end dry-run and run reporting"
created: 2026-07-02
closed: 2026-07-02
closed_commit: 47a890d9c49c5e253eaf87a673b07b524f7ea533
---

**Description:** As a user, I want a trustworthy summary of every sync so I know exactly what happened or would happen.

**Depends on:** US-009, US-010

**Acceptance Criteria:**
- [ ] Every command supports --dry-run producing an accurate change preview
- [ ] Summaries report added / skipped-as-duplicate / unmatched / errored counts per data type
- [ ] Unmatched and errored items listed with enough detail to act on (title, year, reason)
- [ ] Exit code is non-zero if any item errored (distinct from unmatched)
- [ ] cargo test passes covering the summary formatter