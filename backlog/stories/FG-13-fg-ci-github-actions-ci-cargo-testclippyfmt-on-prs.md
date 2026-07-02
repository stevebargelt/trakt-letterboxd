---
id: FG-13
type: story
status: active
title: "FG-CI: GitHub Actions CI (cargo test/clippy/fmt on PRs)"
created: 2026-07-02
---

**Description:** As the developer, I want CI to run on every PR and push to main so branch protection can require green checks before merge.

**Depends on:** FG-1 (needs the Cargo project)

**Acceptance Criteria:**
- [ ] .github/workflows/ci.yml runs on pull_request and push to main
- [ ] Steps: cargo build, cargo test, cargo clippy -D warnings, cargo fmt --check
- [ ] Uses a stable Rust toolchain and caches cargo/registry+target for speed
- [ ] Workflow runs green on its own PR
- [ ] (Follow-up) main branch protection updated to require the CI check