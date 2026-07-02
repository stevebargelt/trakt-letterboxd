---
id: FG-8
type: story
status: done
title: "US-008: Local sync-state store for idempotency"
created: 2026-07-02
closed: 2026-07-02
closed_commit: c74d4971604c871de73433c7d4cd80bef34171eb
---

**Description:** As a user, I want repeated syncs to skip already-synced items so I don't get duplicate entries.

**Depends on:** US-001

**Acceptance Criteria:**
- [ ] A local state file records which items (keyed by TMDB ID + date + type) have been synced in each direction
- [ ] Sync operations consult and update this store
- [ ] A --force flag re-syncs items even if present in state
- [ ] Corrupt/missing state file is handled by rebuilding, not crashing
- [ ] cargo test passes covering skip-existing and force paths