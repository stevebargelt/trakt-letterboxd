**Last session ended 2026-07-02.**

**Where we left off:** Ran a full real-world end-to-end sync on the user's live accounts (Trakt 253myco <-> Letterboxd stevebargelt). Last action: synced the 149-film Trakt watchlist to Letterboxd via the WATCHLIST importer (worked). The core 12-story milestone is DONE and CI-gated; remaining work is robustness/polish tickets surfaced by real use.

**Picked up next:** (all in active list; none are tickets that closed this session)
1. FG-18 (fast, high value) — fix the T->L instructions: the tool tells users to upload the watchlist CSV to /import, which marks want-to-watch as WATCHED. Route it to the watchlist importer instead. Doc/string change.
2. FG-16 + FG-17 (the "skip what already exists on the other side" pair) — FG-16: L->T must skip films already in Trakt history (avoid duplicate plays). FG-17: T->L smart export (skip already-on-Letterboxd, detect bulk-date clusters, date-enrich dateless-watched). Both automate the manual curation done by hand this session. Needed before a safe FULL library auto-sync.
3. FG-15 (optional) — two-pass year-tolerance matching for the ~5% unmatched (Coco/Us/Frontier(s)).

**External state to remember:**
- User has imported REAL data to live accounts this session: 281 curated diary films + 149 watchlist to Letterboxd; 2 films written to Trakt (384->386, kept). Not reversible via the tool.
- User's Letterboxd export dir: ~/Downloads/letterboxd-stevebargelt-2026-07-02-23-28-utc (108 watched, 2 diary, 6 ratings, 1 watchlist, custom "Want" list). Staged CSVs also in ~/Downloads (import-A-clean-dates, watchlist-import, TEST-enrich-5).
- forge invoke runs engineers in Docker; the 8GB Docker VM crashed ~4x under in-container cargo load (symptom: 0-byte result.json + broken build). Recovery: discard partial, verify green baseline, retry. Bump Docker RAM if continuing. See memory [[forge-invoke-docker-kills]].
- Uncommitted backlog bookkeeping sits in the working tree (main is PR-protected so it can't commit directly); it rides along in the next feature PR.

**Decisions worth not relitigating:**
- Safe/ToS-compliant CSV-bridge path chosen over the unofficial Letterboxd internal API (ban risk). Rust stack. Single-user tool.
- Letterboxd watchlist imports via the watchlist page "Import films to watchlist", NOT /import (verified live; FG-18 fixes the tool's wrong instruction).
- Full 108-film L->T sync deferred until FG-16 lands (would inject ~100 duplicate rewatch plays dated to a bulk-logging day).
- Importing better Trakt dates does NOT correct bad Letterboxd dates — Letterboxd diary entries are additive (film+date), so it duplicates rather than overwrites. Only enriches dateless-watched films cleanly.
- Both accounts contain bulk-date junk (Trakt: 86 films dated 2023-09-10) -> date-quality detection is real value (FG-17).
- Left the 2 e2e-test films on Trakt (they're real, correctly-dated watches).

**Shipped (for reference):** FG-1..FG-9 core (scaffold, Trakt OAuth device-flow, Trakt read+write clients, Letterboxd export parser + import generator, matching+rating conversion, sync-state idempotency, sync from-letterboxd). FG-10 sync to-letterboxd. FG-11 best-effort reviews via Trakt notes. FG-12 dry-run+reporting (fixed live-found ratings mislabel). FG-13 GitHub Actions CI (required check on main). FG-14 README. Also FILED this session: FG-15/16/17/18. main = 231 tests, CI-gated, README shipped.
