---
id: FG-17
type: story
status: active
title: "T->L smart export: skip already-on-Letterboxd + detect bulk-date clusters"
created: 2026-07-03
---

**Description:** As a user with an existing Letterboxd library, I want `sync to-letterboxd` to produce an import CSV that only ADDS value (net-new films + date-enrichment) and does not plant junk dates or overwrite good data, so I can import it safely without manual curation.

**Discovered during:** real e2e use (2026-07-02). Doing this by hand for account 253myco revealed the exact logic the tool should automate:
1. **Skip already-present, dated films** (mirror of FG-16 for the T->L side). Requires the user's Letterboxd export as an optional input to know what already exists. Films already in the Letterboxd diary with a date -> skip (avoid duplicate diary entries).
2. **Date-enrichment bucket**: films watched-but-dateless on Letterboxd -> a Trakt date ADDS a proper diary entry cleanly (verified: Letterboxd attaches the date to the existing watched mark, no duplicate). High value.
3. **Bulk-date detection**: a Trakt watched_at shared by many films on one calendar day (e.g. 86 films on 2023-09-10) is almost certainly a bulk-add, not a real watch date. For such films: for NET-NEW, offer blank date (mark watched, no fake diary date); for ENRICH, SKIP (would plant junk date on an already-watched film).
4. Ratings overwrite existing Letterboxd ratings on import -> warn / make optional.

**Real-world split for reference (369 distinct Trakt films vs 108 LB watched):** 260 net-new clean-date, 21 enrich clean-date, 28 net-new bulk-date, 58 enrich bulk-date (skip), 2 already-dated (skip). Only one bulk cluster (2023-09-10, 86 films).

**Acceptance Criteria:**
- [ ] `sync to-letterboxd` accepts an optional Letterboxd export path and excludes already-dated overlap
- [ ] Bulk-date clusters detected (threshold on films-per-day) and handled: net-new -> blank date option; enrich -> skipped
- [ ] Dateless-watched films date-enriched from Trakt
- [ ] Summary reports the buckets (net-new / enriched / skipped-bulk / skipped-existing)
- [ ] Rating overwrite is opt-in or warned
- [ ] Tests cover each bucket; clippy + fmt clean

Related: [[]] FG-16 (L->T mirror).