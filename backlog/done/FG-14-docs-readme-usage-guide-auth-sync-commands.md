---
id: FG-14
type: story
status: done
title: "docs: README + usage guide (auth + sync commands)"
created: 2026-07-02
closed: 2026-07-02
closed_commit: f7ef3de8910c75094c95d2a08f4bab6cfb9d9f69
---

**Description:** As a user/operator, I want a README and usage guide documenting setup (Trakt app registration, config file, env vars) and the auth + sync commands, so the CLI is usable without reading source.

**Why deferred here:** FG-2 made the `auth` command operator-visible, but the sync commands (FG-9/FG-10) are still stubs. A usage guide is worth writing once the core sync flow is functional rather than documenting stubs piecemeal.

**Trigger:** Write/complete this once FG-9 and FG-10 (the sync commands) land.

**Acceptance Criteria:**
- [ ] README covers: what the tool does + the Letterboxd-CSV-bridge constraint (link PRD), install/build, Trakt app registration (client id/secret), config file location + env vars
- [ ] Documents each command: auth, sync from-letterboxd, sync to-letterboxd (incl. the manual Letterboxd upload step for T->L)
- [ ] Notes the safe/ToS-compliant design and non-goals
- [ ] Routed through documentation-maintainer