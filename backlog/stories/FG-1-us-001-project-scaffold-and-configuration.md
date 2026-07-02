---
id: FG-1
type: story
status: active
title: "US-001: Project scaffold and configuration"
created: 2026-07-02
---

**Description:** As the developer, I need a Rust project skeleton and config loading so all later work has a home and a way to read credentials/settings.

**Depends on:** none (foundation)

**Acceptance Criteria:**
- [ ] Cargo project initialized with clap CLI skeleton, dependency manifest, cargo test + clippy/fmt set up
- [ ] Config loaded from a local file and/or env vars: Trakt client ID, Trakt client secret, optional TMDB API key, data directory path
- [ ] Missing required config produces a clear error message, not a panic/stack trace
- [ ] A --help entrypoint runs and lists available commands
- [ ] cargo test passes