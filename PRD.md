# PRD: Trakt ↔ Letterboxd Personal Sync CLI

## Introduction

A personal, single-user command-line tool that keeps a Trakt account and a
Letterboxd account in sync for films — watched history, ratings, watchlist, and
(best-effort) diary dates and reviews.

The design is dictated by a hard external constraint established during research
(`run-trakt-letterboxd-sync-feasibility-b6a1ff`): **Letterboxd has no usable
write or read API for personal projects.** Their API beta explicitly excludes
"private or personal projects," so approval odds are effectively zero. Trakt, by
contrast, offers a fully open OAuth read/write API with no approval gate.

Therefore the tool is **Trakt-API on one side, CSV-file-bridge on the Letterboxd
side**, and every Letterboxd interaction involves one manual browser step:

- **Letterboxd → Trakt:** you export your data ZIP from Letterboxd → the tool
  parses it, matches films, and writes to Trakt via its API (fully automated on
  the Trakt side).
- **Trakt → Letterboxd:** the tool reads Trakt via its API and generates a
  Letterboxd-import CSV → you upload it manually in the Letterboxd web UI.

This is fully compliant with both services' terms of use and carries no
account-ban risk.

## Goals

- Sync **Letterboxd → Trakt** for watched history (with dates), ratings, and
  watchlist, fully automated on the Trakt write side.
- Sync **Trakt → Letterboxd** by generating a valid Letterboxd-import CSV
  (watched date, rating, review, rewatch, tags) with TMDB IDs for exact matching.
- Match films reliably across platforms using **TMDB ID as the canonical bridge**,
  falling back to IMDb ID, then title+year fuzzy match.
- Be **idempotent**: running a sync repeatedly must not create duplicate entries
  on either side.
- Convert ratings correctly between Letterboxd's 0.5–5.0 half-star scale and
  Trakt's 1–10 integer scale (×2 / ÷2).
- Provide a **dry-run** mode that reports what would change without writing.

## Non-Goals

- **No automated writes to Letterboxd.** T→L always ends in a manual CSV upload.
  We will not use Letterboxd's unofficial internal API (ToS violation / ban risk)
  — this was explicitly decided against.
- **No always-on / scheduled service.** The manual Letterboxd browser steps make
  unattended operation impossible; this is an on-demand CLI only.
- **No TV shows or episodes.** Letterboxd is film-only; TV content is out of scope
  entirely.
- **No multi-user support, hosting, or web UI.** Single user, local, hardcoded/
  config-file credentials.
- **No full-fidelity review/diary narrative sync.** Trakt has no first-class
  review field; review text is handled best-effort only (see US-011) and is not
  guaranteed round-trippable. Likes/favorites are out of scope for v1.
- **No conflict-resolution UI.** Idempotency + "skip already-synced" is the
  dedup strategy; there is no interactive merge for divergent edits.

## User Stories

### US-001: Project scaffold and configuration
**Description:** As the developer, I need a project skeleton and config loading so
all later work has a home and a way to read credentials/settings.

**Acceptance Criteria:**
- [ ] Project initialized with chosen stack, dependency manifest, and lint/test setup
- [ ] Config loaded from a local file and/or env vars: Trakt client ID, Trakt client secret, optional TMDB API key, data directory path
- [ ] Missing required config produces a clear error message, not a stack trace
- [ ] A `--help` CLI entrypoint runs and lists available commands
- [ ] Unit tests pass

### US-002: Trakt OAuth device-flow authentication
**Description:** As a user, I want to authorize the tool against my Trakt account
once so it can read and write on my behalf.

**Acceptance Criteria:**
- [ ] `auth` command runs the OAuth 2.0 Device Flow (shows code + verification URL)
- [ ] Access + refresh tokens persisted locally (file permissions restricted to user)
- [ ] Expired access tokens are refreshed automatically before API calls
- [ ] Re-running `auth` cleanly re-authorizes without leaving stale tokens
- [ ] Unit tests pass (token persistence + refresh logic mocked)

### US-003: Trakt read client
**Description:** As the developer, I need to read the user's Trakt data so it can
be exported toward Letterboxd.

**Acceptance Criteria:**
- [ ] Fetch watched history (with `watched_at` dates), ratings, and watchlist
- [ ] Handles pagination across large histories
- [ ] Respects GET rate limit (1,000 / 5-min window) with backoff on HTTP 429
- [ ] Each returned film includes its TMDB and IMDb IDs where available
- [ ] Unit tests pass (HTTP layer mocked)

### US-004: Trakt write client with rate-limit handling
**Description:** As the developer, I need to write history, ratings, and watchlist
to Trakt so Letterboxd data can be imported.

**Acceptance Criteria:**
- [ ] `POST /sync/history`, `/sync/ratings`, `/sync/watchlist` supported
- [ ] Respects write rate limit (1 POST/sec) and honors `Retry-After` on 429
- [ ] Watched entries submit backdated `watched_at` ISO-8601 timestamps
- [ ] Ratings submitted on Trakt's 1–10 scale
- [ ] Unit tests pass (HTTP layer mocked)

### US-005: Letterboxd export CSV parser
**Description:** As the developer, I need to read a Letterboxd data-export ZIP so
its contents can be pushed to Trakt.

**Acceptance Criteria:**
- [ ] Parses `diary.csv`, `ratings.csv`, `watchlist.csv`, and `reviews.csv` from the export
- [ ] Extracts title, year, Letterboxd URI, rating (0.5–5), watched date, rewatch flag, tags, review text
- [ ] Handles UTF-8, quoted fields containing commas, and missing optional columns
- [ ] Malformed rows are skipped with a warning, not a crash
- [ ] Unit tests pass against sample export fixtures

### US-006: Letterboxd import CSV generator
**Description:** As the developer, I need to produce a Letterboxd-compatible import
CSV from Trakt data so the user can upload it.

**Acceptance Criteria:**
- [ ] Emits columns: `Title`, `Year`, `tmdbID`, `WatchedDate` (YYYY-MM-DD), `Rating` (0.5–5), `Rewatch`, `Tags`, `Review`
- [ ] `tmdbID` populated from Trakt data for exact matching; rating ÷2 into half-star scale
- [ ] Output is valid UTF-8 CSV with correct quoting (commas in review text handled)
- [ ] Watchlist emitted as a separate file/section from diary/watched entries
- [ ] Unit tests pass verifying header + a representative row round-trips a Letterboxd import spec

### US-007: Cross-platform film matching and rating conversion
**Description:** As the developer, I need to resolve a film from one platform to the
other reliably so entries land on the correct title.

**Acceptance Criteria:**
- [ ] Given a Letterboxd export row (no TMDB/IMDb ID), resolve to a Trakt film via title+year search, confirmed against TMDB ID where possible
- [ ] Prefer TMDB ID, then IMDb ID, then title+year as the match strategy
- [ ] Rating conversion helpers cover both directions (×2 and ÷2) with half-star handling
- [ ] Unmatched films are collected and reported, not silently dropped
- [ ] Unit tests pass covering a clean match, a fuzzy match, and an unmatched item

### US-008: Local sync-state store for idempotency
**Description:** As a user, I want repeated syncs to skip already-synced items so I
don't get duplicate entries.

**Acceptance Criteria:**
- [ ] A local state file records which items (keyed by TMDB ID + date + type) have been synced in each direction
- [ ] Sync operations consult and update this store
- [ ] A `--force` flag re-syncs items even if present in state
- [ ] Corrupt/missing state file is handled by rebuilding, not crashing
- [ ] Unit tests pass covering skip-existing and force paths

### US-009: `sync from-letterboxd` command (L→T)
**Description:** As a user, I want to push my exported Letterboxd data into Trakt in
one command.

**Acceptance Criteria:**
- [ ] Command takes a path to the Letterboxd export ZIP (or extracted folder)
- [ ] Parses (US-005) → matches (US-007) → writes history/ratings/watchlist to Trakt (US-004), skipping already-synced (US-008)
- [ ] Prints a summary: N watched, N ratings, N watchlist added; M unmatched (listed)
- [ ] `--dry-run` reports the same summary without writing to Trakt
- [ ] Unit/integration tests pass against fixtures with the Trakt client mocked

### US-010: `sync to-letterboxd` command (T→L)
**Description:** As a user, I want to generate a Letterboxd-import CSV from my Trakt
data so I can upload it.

**Acceptance Criteria:**
- [ ] Reads Trakt history/ratings/watchlist (US-003), generates import CSV(s) (US-006), skipping already-exported items (US-008)
- [ ] Writes CSV(s) to the configured data directory and prints their paths
- [ ] Prints clear next-step instructions for the manual Letterboxd upload (which page, which file)
- [ ] `--dry-run` reports counts without writing files
- [ ] Unit/integration tests pass against fixtures with the Trakt client mocked

### US-011: Best-effort diary date and review handling
**Description:** As a user, I want diary watch-dates and review text carried across
where the platforms allow it, with honest limits.

**Acceptance Criteria:**
- [ ] L→T: Letterboxd diary watched-dates populate Trakt history `watched_at`; review text is attached to Trakt notes where a note slot is available, else recorded as "not transferable" in the summary
- [ ] T→L: review text from Trakt notes (if any) is written into the CSV `Review` column
- [ ] The run summary explicitly states how many reviews were transferred vs. skipped and why
- [ ] Unit tests pass covering a review that transfers and one that is reported as skipped

### US-012: End-to-end dry-run and run reporting
**Description:** As a user, I want a trustworthy summary of every sync so I know
exactly what happened or would happen.

**Acceptance Criteria:**
- [ ] Every command supports `--dry-run` producing an accurate change preview
- [ ] Summaries report added / skipped-as-duplicate / unmatched / errored counts per data type
- [ ] Unmatched and errored items are listed with enough detail to act on (title, year, reason)
- [ ] Exit code is non-zero if any item errored (distinct from unmatched)
- [ ] Unit tests pass covering the summary formatter

## Technical Considerations

- **Canonical identity:** TMDB ID is the universal bridge — Trakt exposes it
  natively; Letterboxd accepts it on CSV import and sources all metadata from
  TMDB. Build matching around TMDB IDs. An optional TMDB API key can improve
  reverse (L→T) matching confidence.
- **Rate limits:** Trakt GET = 1,000 / 5-min; writes = 1 / sec. The write client
  must serialize POSTs and honor `Retry-After`.
- **Rating scale:** Letterboxd 0.5–5.0 half-stars ↔ Trakt 1–10 integers (×2 / ÷2).
- **Known matching gaps (~5%):** foreign-title films, same-year remakes, very new
  or obscure releases. These surface in the "unmatched" report rather than fail
  the run.
- **Reference implementations to study:** `f0e/letterboxd-trakt-sync` (L→T,
  Python, active) for the export→Trakt path; `bbeesley/trakt-to-letterboxd` for
  the Trakt→CSV path. These are Python — study their *approach* (matching, scale
  conversion, endpoint use), not their code. Do not adopt any tool's
  unofficial-Letterboxd-API code.
- **Implementation stack: Rust.** Single self-contained binary, strong typing for
  the API/CSV data shapes, easy local distribution. Suggested crates (to confirm
  at US-001): `clap` (CLI), `serde` + `csv` (CSV parse/emit), `reqwest` + `tokio`
  (HTTP), `serde_json` (Trakt payloads). "Unit tests pass" throughout means
  `cargo test`; treat `cargo clippy`/`cargo fmt` as the lint gate.

## Source

Feasibility research: forge run `run-trakt-letterboxd-sync-feasibility-b6a1ff`
(2026-07-02). Key sources: Trakt API docs + 2026 limits forum post; Letterboxd
API beta page (personal-project exclusion); Letterboxd import/export format docs;
Letterboxd ToS.
