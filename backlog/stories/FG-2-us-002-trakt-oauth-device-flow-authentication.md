---
id: FG-2
type: story
status: active
title: "US-002: Trakt OAuth device-flow authentication"
created: 2026-07-02
---

**Description:** As a user, I want to authorize the tool against my Trakt account once so it can read and write on my behalf.

**Depends on:** US-001

**Acceptance Criteria:**
- [ ] auth command runs OAuth 2.0 Device Flow (shows code + verification URL)
- [ ] Access + refresh tokens persisted locally with user-restricted file permissions
- [ ] Expired access tokens refreshed automatically before API calls
- [ ] Re-running auth cleanly re-authorizes without leaving stale tokens
- [ ] cargo test passes (token persistence + refresh logic mocked)