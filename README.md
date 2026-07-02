# trakt-letterboxd

A personal CLI that syncs films between Trakt and Letterboxd — watched history, ratings, watchlist, and best-effort diary reviews.

## The key constraint

Letterboxd has no usable read/write API for personal projects. As a result, the two sync directions work differently:

- **Letterboxd → Trakt**: you export your data from Letterboxd (Settings → Data → Export Your Data), run `sync from-letterboxd` pointing at the ZIP, and the tool writes directly to Trakt via its API. Fully automated on the Trakt side.
- **Trakt → Letterboxd**: the tool reads Trakt via its API and generates two CSV files. You then upload them manually at [letterboxd.com/import](https://letterboxd.com/import). One manual browser step required.

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
- `--force`: re-sync all items, ignoring the local state file (bypasses dedup)

**What it syncs**: watched history (with backdated `watched_at` dates), ratings (converted to Trakt's 1–10 scale), watchlist entries, and review text as Trakt notes (best-effort; see [Review handling](#review-handling)).

**Output summary**:
```
Letterboxd → Trakt sync complete

  Watched history:  12 added, 3 skipped (already synced)
  Ratings:          10 added, 2 skipped (already synced)
  Watchlist:        5 added, 0 skipped (already synced)
  Reviews:          4 transferred, 1 skipped (over limit), 0 skipped (film unmatched), 0 errored

  Unmatched films (2):
    - Obscure Title (1974): no exact title+year match in Trakt search
    ...
```

Exit code is non-zero only when items appear in the **errored** list. Unmatched films (no write attempted) do not trigger a non-zero exit.

### `sync to-letterboxd [--dry-run] [--force]`

Read Trakt data and generate Letterboxd import CSVs.

- `--dry-run`: fetch Trakt data and report counts without writing any files or updating sync state
- `--force`: re-export all items, ignoring previously exported items

**Output files** written to `data_dir`:
- `letterboxd-diary-import.csv` — watched history with dates, ratings, and any review text
- `letterboxd-watchlist-import.csv` — watchlist entries

**Output summary**:
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

Next steps: Upload these files at https://letterboxd.com/import/ — diary file first, then watchlist.
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

1. Run:
   ```sh
   trakt-letterboxd sync to-letterboxd
   ```
2. Go to [letterboxd.com/import](https://letterboxd.com/import).
3. Upload the **diary CSV first**, then the **watchlist CSV**.

## How it behaves

**Idempotent**: a local state file (`sync_state.json` in `data_dir`) records synced items keyed by TMDB ID, type, and date. Re-running does not create duplicates. Use `--force` to override.

**Film matching**: TMDB ID is used as the canonical bridge — Trakt exposes it natively and Letterboxd accepts it on import. When TMDB ID is unavailable, the tool falls back to IMDb ID, then title+year. A minority of films (foreign titles, same-year remakes, very obscure releases) may go unmatched; these are listed in the run summary, not silently dropped.

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
