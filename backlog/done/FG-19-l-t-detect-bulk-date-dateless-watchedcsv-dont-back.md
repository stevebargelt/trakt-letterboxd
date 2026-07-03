---
id: FG-19
type: story
status: done
title: "L->T: detect bulk-date / dateless watched.csv, don't backdate to import-day (mirror FG-17)"
created: 2026-07-03
closed: 2026-07-03
closed_commit: 14cad23
---

**Description:** As a user, when I sync Letterboxd -> Trakt I do NOT want films from a Letterboxd bulk-import day to be written to Trakt stamped with that import date, so my Trakt history is not polluted with a fake bulk-date cluster.

**Discovered during:** real-data validation of FG-16 (2026-07-02, follow-on session). The user's Letterboxd `watched.csv` had 106 of 108 films sharing the date `2023-06-16` — a Letterboxd "mark as watched" bulk backfill, not real watch dates. Of those, ~26 were not yet on Trakt. The current L->T sync backdates dateless `watched.csv` films to their logged date, so writing them would plant ~26 films dated 2023-06-16 on Trakt — a synthetic bulk cluster.

**Root issue:** `watched.csv` entries carry a Letterboxd *add* date (the CSV `Date` column), NOT a real watch date. When many share one calendar day it is almost certainly a bulk add. FG-17 already solved the equivalent problem on the T->L side (bulk-date cluster detection: a day shared by >= BULK_DATE_THRESHOLD films is treated as a bulk-add; net-new -> blank date, enrich -> skip). The L->T direction has NO equivalent — it blindly backdates.

**Approach:** Mirror FG-17 for the L->T direction. Detect bulk-date clusters in the Letterboxd `watched.csv` `Date` column via a films-per-day threshold (reuse/share the FG-17 `BULK_DATE_THRESHOLD = 10` constant). For films on a detected bulk day: either skip, OR mark-watched on Trakt with NO `watched_at` date (opt-in), rather than backdating to the import day. Keep genuinely-dated diary.csv entries syncing normally.

Note: diary.csv (real dated entries) is unaffected; this targets the dateless/bulk `watched.csv` path only. Interacts with FG-16 (already skips films already on Trakt) — bulk-date handling applies to the net-new remainder.

**Acceptance Criteria:**
- Bulk-date clusters in `watched.csv` detected via a films-per-day threshold (shared with / consistent with FG-17)
- Dateless / bulk-day `watched.csv` films are NOT backdated to the import day on Trakt
- Behavior for such films is opt-in: default skip (or default no-date), with a flag to choose — decide the exact default at planning/gate
- diary.csv real-dated entries continue to sync with their real dates
- Summary reports bulk-date-skipped / no-date-marked films distinctly
- Tests cover: a bulk cluster in watched.csv is detected; those films are not backdated; a genuinely-dated diary entry still syncs its date; threshold boundary
- cargo test / clippy / fmt clean

**Related:** FG-17 (T->L mirror — the direction this ports), FG-16 (L->T already-on-Trakt skip).
