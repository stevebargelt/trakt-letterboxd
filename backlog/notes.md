**Last session ended 2026-07-02.**

**Where we left off:** Backlog is EMPTY — all 12 core stories + all robustness tickets shipped. This session shipped FG-18/16/17/15, then discovered + shipped FG-19, and validated BOTH sync directions on the user's live data. No code work is queued; next moves are operational/validation or new tickets from real use.

**Picked up next:** (no active tickets — these are non-ticket threads)
1. (optional) Real L->T write — now SAFE after FG-16+FG-19: a real `sync from-letterboxd` would write 0 watched plays (83 skipped already-on-Trakt, 26 skipped bulk-import-date) and ~6 ratings. User deferred it this session. If doing it: peek at the 6 ratings first, then run without --dry-run.
2. (optional) Real T->L smart export — REQUIRES A FRESH Letterboxd export. The on-disk export (~/Downloads/letterboxd-stevebargelt-2026-07-02-23-28-utc) is a PRE-import snapshot; user imported ~281 diary films to LB last session, so a current export would show far more already-dated/enriched and far fewer net-new. Re-export from Letterboxd first, then `sync to-letterboxd --letterboxd-export <fresh> --dry-run`.
3. (candidate new ticket, not filed) Title-matching for the 2 persistent unmatched films — `Frontier(s) (2007)` (punctuation) and `Us (2019)` (ultra-short/ambiguous title). This is a TITLE normalization issue (not year — FG-15 already handles year±1). Would be the title-side mirror of FG-15.

**External state to remember:**
- Trakt creds config: `/Users/stevebargelt/code/trakt-letterboxd/config.toml` (PROJECT ROOT, not ~/.config). Pass `--config config.toml`. (Spent time this session relocating it — it is NOT in the default ~/.config path.)
- Trakt OAuth token: ~/.local/share/trakt-letterboxd/tokens.json — was valid ~7 days from Jul 2, so likely EXPIRES ~Jul 9. Re-auth (`trakt-letterboxd auth`) if API calls 401.
- sync_state.json (~/.local/share/trakt-letterboxd/) holds 535 exported items from last session's T->L run. Consequence: T->L dry-runs show "already exported: 535 skipped" and hide the buckets — use `--force --dry-run` to see the FG-17 classification (safe: dry-run writes nothing and does not touch sync_state).
- Letterboxd export on disk is PRE-import: 108 watched (106 dated 2023-06-16 = a 2023 bulk backfill), 2 diary, 6 ratings, 1 watchlist.
- Trakt API requires a User-Agent header (403 without it) — relevant only if hand-calling the API for diagnostics; the tool sets it correctly.
- FG-17 real-data buckets (--force, pre-import export): 267 net-new-clean / 28 net-new-bulk / 30 enriched / 59 skipped-bulk / 2 already-dated — matches the session oracle on the distinctive buckets. FG-16 real-data: 83 skipped already-on-Trakt. FG-19 real-data: 26 skipped bulk-import-date (were being written as fake 2023-06-16 plays before FG-19).

**Decisions worth not relitigating:**
- The 26 L->T "net-new" films are a 2023-06-16 Letterboxd backfill (junk add-date, not real watch dates) — correctly NOT written to Trakt. This is what motivated FG-19.
- FG-19 behavior = SKIP + report bulk-day films (no date, no opt-in flag). Trakt history requires a watched_at, so a true "no-date" add is impossible; skipping is the clean choice. Owner-confirmed.
- Bulk-date threshold = 10, shared by BOTH directions via src/constants.rs::BULK_DATE_THRESHOLD (user asked for "same logic both ways").
- FG-17 ratings = opt-in via --include-ratings (default off); default emits blank Rating column so a first import never overwrites existing LB ratings.
- FG-17 pipeline caught a real cross-run bug (sync_state was marked for skipped/non-emitted films) — fixed + regression-tested before merge. Don't reintroduce: only mark sync_state for rows actually emitted.
- Did NOT pursue the real L->T write or a fresh T->L export this session — user validated the logic and moved on.

**Shipped (for reference):** FG-18 (route watchlist CSV to watchlist importer, not /import). FG-16 (L->T skip already-on-Trakt). FG-17 (T->L smart export: LB-export input, five-bucket classification, bulk-date detection, ratings opt-in). FG-15 (two-pass year±1 tolerance matching w/ near-year warnings). FG-19 (L->T bulk-import-date skip, mirror of FG-17, shared BULK_DATE_THRESHOLD). All CI-gated, README reconciled each. Earlier: FG-1..FG-14 core.
