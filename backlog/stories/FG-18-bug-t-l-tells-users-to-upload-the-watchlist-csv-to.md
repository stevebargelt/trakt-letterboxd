---
id: FG-18
type: story
status: active
title: "bug: T->L tells users to upload the watchlist CSV to /import (marks it watched)"
created: 2026-07-03
---

**Description:** The `sync to-letterboxd` summary and README instruct users to upload BOTH generated CSVs at https://letterboxd.com/import. That is WRONG for the watchlist file: the main import page marks all films WATCHED, so following the instruction would mark every want-to-watch film as watched instead of adding it to the watchlist.

**Correct behavior (verified 2026-07-02):** Letterboxd has a SEPARATE watchlist importer. The diary/watched CSV goes to letterboxd.com/import (marks watched). The watchlist CSV must be imported from the user's WATCHLIST page -> sidebar "Import films to watchlist" -> attach -> "Add films to watchlist". These are two distinct operations/paths.

**Impact:** data corruption risk — users following current instructions convert their watchlist into watched history.

**Fix:**
- Update the print_to_letterboxd_summary next-steps text: diary CSV -> /import; watchlist CSV -> watchlist page "Import films to watchlist" (give the exact steps/label).
- Update README T->L workflow section accordingly.
- Consider renaming the watchlist output file to make its destination obvious.

**Acceptance Criteria:**
- [ ] Summary + README clearly route the watchlist CSV to the watchlist importer, NOT /import
- [ ] Wording names the exact Letterboxd UI step
- [ ] cargo test / clippy / fmt clean (doc/string change)