# trakt-letterboxd

A personal CLI that syncs films between Trakt and Letterboxd — watched history, ratings, watchlist, and best-effort diary reviews.

## The key constraint

Letterboxd has no usable read/write API for personal projects. As a result, the two sync directions work differently:

- **Letterboxd → Trakt**: you export your data from Letterboxd (Settings → Data → Export Your Data), run `sync from-letterboxd` pointing at the ZIP, and the tool writes directly to Trakt via its API. Fully automated on the Trakt side.
- **Trakt → Letterboxd**: the tool reads Trakt via its API and generates two CSV files — a diary/watched CSV and a watchlist CSV. Each goes to a different Letterboxd importer: the diary CSV to [letterboxd.com/import](https://letterboxd.com/import) (marks films as watched), and the watchlist CSV via the **Import films to watchlist** sidebar on your Letterboxd Watchlist page. Two manual browser steps required.

No scraping, no unofficial APIs. Fully compliant with both services' terms of use.

## Setup

### 1. Build

Requires Rust (stable). Install via [rustup.rs](https://rustup.rs).

```sh
cargo build --release
# binary at: target/release/trakt-letterboxd
```

### 2. Register a Trakt API application

Go to [trakt.tv/oauth/applications](https://trakt.tv/oauth/applications) and create a new application.

- **Redirect URI**: set to `urn:ietf:wg:oauth:2.0:oob`
- Note the **Client ID** and **Client Secret** — you will need them for config.

### 3. Configure

Create `~/.config/trakt-letterboxd/config.toml`:

```toml
trakt_client_id     = "your_client_id"
trakt_client_secret = "your_client_secret"

# Optional: improves film matching accuracy for Letterboxd → Trakt syncs
# tmdb_api_key = "your_tmdb_key"

# Optional: where CSV output and sync state are stored (default: ~/.local/share/trakt-letterboxd)
# data_dir = "/path/to/data"
```

**Config file location** (highest to lowest precedence):
1. `--config <path>` CLI flag
2. `TRAKT_CONFIG_FILE` environment variable
3. `~/.config/trakt-letterboxd/config.toml`

**Environment variable overrides** (take precedence over file values):

| Variable              | Config key             |
|-----------------------|------------------------|
| `TRAKT_CLIENT_ID`     | `trakt_client_id`      |
| `TRAKT_CLIENT_SECRET` | `trakt_client_secret`  |
| `TMDB_API_KEY`        | `tmdb_api_key`         |
| `DATA_DIR`            | `data_dir`             |

> **Security note**: `config.toml` and `tokens.json` contain secrets. Both are gitignored. `tokens.json` is written with mode `0600`.

### 4. Authorize

```sh
trakt-letterboxd auth
```

This runs the OAuth 2.0 device flow. The tool prints a URL and a short code:

```
  Visit:      https://trakt.tv/activate
  Enter code: ABCD-1234

Waiting for authorization...
```

Open the URL in a browser, enter the code, and approve the request. The tool saves tokens to `<data_dir>/tokens.json` and refreshes them automatically before they expire.

## Commands

```
trakt-letterboxd [--config <path>] <command>
```

### `auth`

Authorize with Trakt via OAuth device flow. Re-run at any time to re-authorize.

### `trakt-status`

Show the authenticated Trakt username and current movie counts (watched, rated, watchlist).

### `sync from-letterboxd <path> [--dry-run] [--force]`

Import a Letterboxd export into Trakt.

- `<path>`: path to the Letterboxd export ZIP or an extracted directory
- `--dry-run`: parse and report what would be synced without writing anything to Trakt or updating sync state
- `--force`: re-sync items that are already recorded in local sync state (bypasses local dedup). Films already present in your Trakt watched history are **always** skipped regardless of `--force` — see [Duplicate play prevention](#duplicate-play-prevention) below.

**What it syncs**: watched history (with backdated `watched_at` dates), ratings (converted to Trakt's 1–10 scale), watchlist entries, and review text as Trakt notes (best-effort; see [Review handling](#review-handling)).

**Output summary**:
```
Letterboxd → Trakt sync complete

  Watched history:  12 added, 1 skipped (already on Trakt), 86 skipped (bulk import date), 3 skipped (already synced)
  Ratings:          10 added, 2 skipped (already synced)
  Watchlist:        5 added, 0 skipped (already synced)
  Reviews:          4 transferred, 1 skipped (over limit), 0 skipped (film unmatched), 0 errored

  Near-year matches (1) — please verify:
    - 'Coco' (year 2018) matched Trakt year 2017 — verify this is the same film

  Unmatched films (1):
    - Obscure Title (1974): no exact title+year match in Trakt search
```

The watched-history line has four distinct counts:
- **added** — written to Trakt this run
- **skipped (already on Trakt)** — film already in your Trakt watched history; skipped to prevent a duplicate play entry (not overridable by `--force`)
- **skipped (bulk import date)** — the `Date` column in `watched.csv` is a Letterboxd add date, not a real watch date; when 10 or more `watched.csv` films share one date it is treated as a bulk import and those films are skipped rather than backdated (same threshold used by the T→L direction — see [Bulk-date detection](#how-it-behaves))
- **skipped (already synced)** — recorded in local sync state from a prior run; skipped by idempotency logic (`--force` bypasses this)

Exit code is non-zero only when items appear in the **errored** list. Unmatched films (no write attempted) do not trigger a non-zero exit.

### `sync to-letterboxd [--dry-run] [--force] [--letterboxd-export <PATH>] [--include-ratings]`

Read Trakt data and generate Letterboxd import CSVs.

- `--dry-run`: fetch Trakt data and report counts without writing any files or updating sync state
- `--force`: re-export all items, ignoring previously exported items. Clears local sync state for the T→L direction; does **not** override the already-dated skip imposed by `--letterboxd-export`.
- `--letterboxd-export <PATH>`: path to the user's Letterboxd export directory or ZIP. When provided, each Trakt film is classified against the existing LB library so only net-new and date-enrichable films are emitted. Omitting this flag preserves the previous behavior (all SyncState-unrecorded films are exported).
- `--include-ratings`: write Trakt ratings into the `Rating` column of the diary CSV. **Off by default** — Letterboxd's importer overwrites existing ratings with no undo.

**Output files** written to `data_dir`:
- `letterboxd-diary-import.csv` — watched history with dates and any review text; the `Rating` column is blank by default (pass `--include-ratings` to populate it)
- `letterboxd-watchlist-import.csv` — watchlist entries

**Output summary** (without `--letterboxd-export`):
```
Trakt → Letterboxd export complete

  Diary rows:               47
  Distinct rated films:     38
  Diary rows with a rating: 42 (may include rewatches of rated films)
  Reviews in diary:         5
  Watchlist rows:           12
  Already exported:         3 skipped

  Diary CSV:     /home/user/.local/share/trakt-letterboxd/letterboxd-diary-import.csv
  Watchlist CSV: /home/user/.local/share/trakt-letterboxd/letterboxd-watchlist-import.csv

Next steps:
  1. Diary CSV   → https://letterboxd.com/import/ (marks films as watched)
  2. Watchlist CSV → Your Letterboxd Watchlist page → sidebar 'Import films to watchlist' → attach the CSV → 'Add films to watchlist'
```

When `--letterboxd-export` is provided, five classification-bucket counts also appear:
```
  Net-new (clean date):       240
  Net-new (bulk date, blank): 28
  Enriched (date added):      21
  Skipped (bulk+dateless):    58
  Skipped (already dated):    2
```

When `--include-ratings` is omitted and the diary contains rated films, a note appears:
```
  Ratings omitted from CSV (pass --include-ratings to include; Letterboxd import overwrites existing ratings)
```

## Typical workflows

### Letterboxd → Trakt

1. In Letterboxd: **Settings → Data → Export Your Data** — download the ZIP.
2. Run:
   ```sh
   trakt-letterboxd sync from-letterboxd ~/Downloads/letterboxd-export.zip
   ```
3. Check the summary for unmatched or errored films.

### Trakt → Letterboxd

**Basic** (exports everything not yet in local sync state):
1. Run:
   ```sh
   trakt-letterboxd sync to-letterboxd
   ```
2. Upload the **diary CSV** at [letterboxd.com/import](https://letterboxd.com/import) (marks films as watched).
3. Upload the **watchlist CSV** via your Letterboxd Watchlist page → sidebar **Import films to watchlist** → attach the CSV → **Add films to watchlist**.

**With smart deduplication** (skip films already in your Letterboxd library):
1. In Letterboxd: **Settings → Data → Export Your Data** — download the ZIP.
2. Run:
   ```sh
   trakt-letterboxd sync to-letterboxd --letterboxd-export ~/Downloads/letterboxd-export.zip
   ```
3. Upload the CSVs as in steps 2–3 of the basic workflow above.

To include ratings in the diary CSV, add `--include-ratings` to either invocation. **Caution:** Letterboxd's importer overwrites existing ratings with no undo.

## How it behaves

**Idempotent**: a local state file (`sync_state.json` in `data_dir`) records synced items keyed by TMDB ID, type, and date. Re-running does not create duplicates. Use `--force` to override local state.

**Duplicate play prevention**: before writing watched-history entries, `sync from-letterboxd` fetches your current Trakt watched history and skips any film already present (matched by TMDB ID). This prevents phantom rewatches when Letterboxd's `watched.csv` and your Trakt history overlap. The skip is not overridable by `--force` — `--force` clears only the local state file, not the live Trakt account check.

**Smart deduplication** (`--letterboxd-export`): when a Letterboxd export path is provided, each Trakt film is classified by normalized title + year into one of five buckets:

- **Net-new (clean date)** — not in LB, watch day has fewer than 10 films: emitted with the Trakt watch date.
- **Net-new (bulk date, blank)** — not in LB, watch day has 10 or more films (bulk-add): emitted with a **blank** `WatchedDate` to avoid creating a fake diary date.
- **Enriched** — dateless in LB (no watch date), and the watch day is clean: emitted with the Trakt date to add a real diary date.
- **Skipped (already dated)** — already has a watch date in LB: not emitted. Not recorded in sync state, so it re-appears on the next run if the LB export changes.
- **Skipped (bulk+dateless)** — dateless in LB and the watch day is a bulk day: not emitted to avoid planting a fake diary date. Also not recorded in sync state.

`--force` clears local sync state but does **not** override the already-dated skip. Only films actually emitted to the diary CSV are recorded in sync state.

**Bulk-date detection**: the same threshold (10 or more films on a single calendar day) is applied in both sync directions to avoid planting fake diary dates.
- **L→T** (`watched.csv`): the `Date` field is a Letterboxd *add* date, not a real watch date. When 10 or more `watched.csv` entries share one date, that date is flagged as a bulk import. Those films are skipped entirely — not written to Trakt — so your Trakt history is not polluted with a synthetic same-day cluster. `diary.csv` entries have real `WatchedDate` values and are unaffected.
- **T→L** (`--letterboxd-export`): a calendar day on which 10 or more Trakt films were watched is treated as a bulk import event. Net-new films on such days are emitted with a blank `WatchedDate`; dateless LB films on bulk days are skipped.

**Ratings opt-in** (`--include-ratings`): the `Rating` column of the diary CSV is blank by default. Letterboxd's importer overwrites existing ratings with no undo, so ratings are excluded unless `--include-ratings` is passed explicitly.

**Film matching**: TMDB ID is used as the canonical bridge — Trakt exposes it natively and Letterboxd accepts it on import. When TMDB ID is unavailable, the tool falls back to IMDb ID, then title+year using a two-pass strategy:

1. **Pass 1 (exact)** — title + exact year must match. This is the authoritative path; exact matches produce no warning.
2. **Pass 2 (year ±1 tolerance)** — only films that failed the exact match are retried with year−1 and year+1. The title must still match exactly; a different title one year apart does not match. Any match found here is flagged with a warning and listed in the run summary under **"Near-year matches — please verify"** so you can confirm the tool found the right film. This recovers arthouse titles whose festival-premiere year and wide-release year differ by one (e.g. *Coco*, *Us*).

Films that fail both passes are listed in **"Unmatched films"** in the run summary and are never silently dropped.

**Rating scale**: Letterboxd uses 0.5–5.0 half-stars; Trakt uses integers 1–10. Conversion is ×2 in each direction (e.g. Letterboxd 4.5 → Trakt 9, Trakt 8 → Letterboxd 4.0).

**Review handling**: Reviews are transferred on a best-effort basis via Trakt notes.
- **L→T**: review text from Letterboxd's `reviews.csv` is posted to Trakt's notes API. Free-tier Trakt accounts have a note limit; reviews beyond that limit are counted as "skipped (over limit)" in the summary but do not cause the sync to fail.
- **T→L**: Trakt notes for watched films are written into the `Review` column of the diary CSV.
- Full round-trip fidelity is not guaranteed.

**Exit codes**: non-zero on real errors (API failures, file write failures). Unmatched films and over-limit review skips are informational — they do not cause a non-zero exit.

## Limitations

- No automated writes to Letterboxd. The T→L direction always ends with a manual CSV upload.
- Single-user, local tool only. No scheduled or always-on service.
- Films only. Letterboxd is film-only; TV shows and episodes are out of scope.
- No conflict resolution. The tool skips already-synced items; it does not merge divergent edits between the two services.
- Review fidelity is best-effort. See [PRD.md](PRD.md) for the full design rationale and non-goals.

## License

MIT — see [LICENSE](LICENSE).
