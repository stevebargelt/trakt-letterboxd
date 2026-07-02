---
id: FG-15
type: story
status: active
title: "matching: two-pass year tolerance (recover festival-year mismatches)"
created: 2026-07-02
---

**Description:** As a user, I want films whose Letterboxd year differs by 1 from Trakt (festival premiere vs wide release) to still match, so ~5% of arthouse titles stop landing in the unmatched list every sync.

**Discovered during:** FG-7 verification. FG-7 ships with EXACT-year matching (its AC), which is correct and reports unmatched films visibly. This is a NEW enhancement, not unfinished FG-7 scope.

**Approach (recommended by test-engineer):** two-pass in src/matching.rs — try exact year first; on no match, retry with year +/- 1 and emit a warning so the near-match is visible. Keeps the happy path strict to avoid false-positive matches on adjacent-year sequels/reboots.

**Acceptance Criteria:**
- [ ] On exact-year miss, matching retries year +/-1 and matches an otherwise-identical title
- [ ] A +/-1 match emits a warning (visible in the sync summary) rather than a silent match
- [ ] Adjacent-year DIFFERENT titles still do NOT false-match
- [ ] Existing exact-match tests stay green; new tests cover the +/-1 recovery and the false-positive guard
- [ ] cargo test / clippy / fmt clean