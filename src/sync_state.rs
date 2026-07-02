// Public API consumed by FG-9/FG-10; suppress dead_code warnings until those callers land.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub enum Direction {
    LetterboxdToTrakt,
    TraktToLetterboxd,
}

impl Direction {
    fn as_str(&self) -> &'static str {
        match self {
            Direction::LetterboxdToTrakt => "L2T",
            Direction::TraktToLetterboxd => "T2L",
        }
    }
}

pub enum ItemType {
    Watched,
    Rating,
    Watchlist,
}

impl ItemType {
    fn as_str(&self) -> &'static str {
        match self {
            ItemType::Watched => "watched",
            ItemType::Rating => "rating",
            ItemType::Watchlist => "watchlist",
        }
    }
}

/// Stable film identifier. Prefer Tmdb when available; fall back to TitleYear for films
/// without a TMDB ID so they still get a deterministic dedup key.
pub enum ItemRef {
    Tmdb(u64),
    TitleYear(String, u16),
}

impl ItemRef {
    fn key_segment(&self) -> String {
        match self {
            ItemRef::Tmdb(id) => format!("tmdb:{id}"),
            ItemRef::TitleYear(title, year) => {
                let normalized: String = title
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase()
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '_' })
                    .collect();
                format!("title:{normalized}:{year}")
            }
        }
    }
}

/// Composite key identifying one synced item in one direction.
/// Serialized as "L2T|watched|tmdb:12345|2024-01-15".
/// For watchlist (set-membership, no specific date), pass an empty string for `date`.
///
/// # Using --force
/// Callers implementing --force should skip the `contains()` check entirely.
/// To reset one direction's entire history, call `clear_direction()`.
pub struct SyncKey {
    pub direction: Direction,
    pub item_type: ItemType,
    pub item_ref: ItemRef,
    /// YYYY-MM-DD, or empty for watchlist entries.
    pub date: String,
}

impl SyncKey {
    pub fn new(
        direction: Direction,
        item_type: ItemType,
        item_ref: ItemRef,
        date: impl Into<String>,
    ) -> Self {
        SyncKey {
            direction,
            item_type,
            item_ref,
            date: date.into(),
        }
    }

    fn as_key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.direction.as_str(),
            self.item_type.as_str(),
            self.item_ref.key_segment(),
            self.date,
        )
    }
}

#[derive(Serialize, Deserialize, Default)]
struct SyncStateData {
    synced: HashSet<String>,
}

/// Persistent idempotency store. Tracks which items have been synced in each direction.
pub struct SyncState {
    data: SyncStateData,
}

fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join("sync_state.json")
}

impl SyncState {
    /// Load from disk. Missing or corrupt file returns a fresh empty state (never panics).
    pub fn load(data_dir: &Path) -> Self {
        match std::fs::read_to_string(state_path(data_dir)) {
            Err(_) => SyncState {
                data: SyncStateData::default(),
            },
            Ok(content) => match serde_json::from_str::<SyncStateData>(&content) {
                Ok(data) => SyncState { data },
                Err(_) => {
                    eprintln!("warning: sync_state.json is corrupt; rebuilding from scratch");
                    SyncState {
                        data: SyncStateData::default(),
                    }
                }
            },
        }
    }

    /// Returns true if this item has already been synced in the given direction.
    pub fn contains(&self, key: &SyncKey) -> bool {
        self.data.synced.contains(&key.as_key())
    }

    /// Record an item as synced.
    pub fn mark(&mut self, key: SyncKey) {
        self.data.synced.insert(key.as_key());
    }

    /// Atomically write state to disk with 0600 permissions.
    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("failed to create data dir: {e}"))?;

        let dest = state_path(data_dir);
        let tmp = data_dir.join("sync_state.json.tmp");

        let json = serde_json::to_string_pretty(&self.data).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, &json).map_err(|e| format!("failed to write state file: {e}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("failed to set state file permissions: {e}"))?;
        }

        std::fs::rename(&tmp, &dest).map_err(|e| format!("failed to rename state file: {e}"))?;
        Ok(())
    }

    /// Number of tracked entries (useful for run summaries).
    pub fn len(&self) -> usize {
        self.data.synced.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.synced.is_empty()
    }

    /// Iterate over raw key strings (for reporting/debugging).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.data.synced.iter().map(String::as_str)
    }

    /// Remove all entries for a given direction. Use this to implement --force per-direction resets.
    pub fn clear_direction(&mut self, direction: &Direction) {
        let prefix = format!("{}|", direction.as_str());
        self.data.synced.retain(|k| !k.starts_with(&prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn watched_l2t(id: u64, date: &str) -> SyncKey {
        SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(id),
            date,
        )
    }

    fn watched_t2l(id: u64, date: &str) -> SyncKey {
        SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Watched,
            ItemRef::Tmdb(id),
            date,
        )
    }

    #[test]
    fn mark_and_contains_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        let key = watched_l2t(12345, "2024-01-15");
        assert!(!state.contains(&key));

        state.mark(watched_l2t(12345, "2024-01-15"));
        assert!(state.contains(&watched_l2t(12345, "2024-01-15")));
    }

    #[test]
    fn persistence_round_trip() {
        let dir = TempDir::new().unwrap();

        {
            let mut state = SyncState::load(dir.path());
            state.mark(watched_l2t(42, "2024-06-01"));
            state.mark(watched_l2t(99, "2024-06-02"));
            state.save(dir.path()).unwrap();
        }

        let state = SyncState::load(dir.path());
        assert!(state.contains(&watched_l2t(42, "2024-06-01")));
        assert!(state.contains(&watched_l2t(99, "2024-06-02")));
        assert!(!state.contains(&watched_l2t(1, "2024-06-01")));
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn missing_file_returns_empty_state() {
        let dir = TempDir::new().unwrap();
        let state = SyncState::load(dir.path());
        assert!(state.is_empty());
    }

    #[test]
    fn corrupt_file_returns_empty_state_without_panic() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("sync_state.json"), b"{{not valid json}}").unwrap();

        let state = SyncState::load(dir.path());
        assert!(state.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn state_file_has_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());
        state.mark(watched_l2t(1, "2024-01-01"));
        state.save(dir.path()).unwrap();

        let mode = std::fs::metadata(dir.path().join("sync_state.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn tmdb_key_and_title_year_key_are_distinct() {
        let tmdb_key = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(12345),
            "2024-01-15",
        );
        let title_key = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear("The Matrix".to_string(), 1999),
            "2024-01-15",
        );

        let mut state = SyncState::load(TempDir::new().unwrap().path());
        state.mark(tmdb_key);
        assert!(!state.contains(&title_key));
    }

    #[test]
    fn same_item_two_directions_tracked_independently() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        state.mark(watched_l2t(77, "2024-03-01"));

        assert!(state.contains(&watched_l2t(77, "2024-03-01")));
        assert!(!state.contains(&watched_t2l(77, "2024-03-01")));

        state.mark(watched_t2l(77, "2024-03-01"));
        assert!(state.contains(&watched_t2l(77, "2024-03-01")));
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn clear_direction_removes_only_that_direction() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        state.mark(watched_l2t(1, "2024-01-01"));
        state.mark(watched_t2l(2, "2024-01-02"));

        state.clear_direction(&Direction::LetterboxdToTrakt);

        assert!(!state.contains(&watched_l2t(1, "2024-01-01")));
        assert!(state.contains(&watched_t2l(2, "2024-01-02")));
    }

    #[test]
    fn watchlist_entry_with_empty_date() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        let key = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watchlist,
            ItemRef::Tmdb(500),
            "",
        );
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watchlist,
            ItemRef::Tmdb(500),
            "",
        ));
        assert!(state.contains(&key));
    }

    #[test]
    fn title_year_normalization_is_stable() {
        let k1 = SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Rating,
            ItemRef::TitleYear("The Matrix".to_string(), 1999),
            "2024-01-01",
        );
        let k2 = SyncKey::new(
            Direction::TraktToLetterboxd,
            ItemType::Rating,
            ItemRef::TitleYear("The Matrix".to_string(), 1999),
            "2024-01-01",
        );
        assert_eq!(k1.as_key(), k2.as_key());
    }

    // --- Gap tests added for FG-8 verify phase ---

    #[test]
    fn title_year_whitespace_normalized_to_same_key() {
        let canonical = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear("The Matrix".to_string(), 1999),
            "1999-03-31",
        );
        let trailing_space = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear("the matrix ".to_string(), 1999),
            "1999-03-31",
        );
        let double_space = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear("the  matrix".to_string(), 1999),
            "1999-03-31",
        );
        assert_eq!(
            canonical.as_key(),
            trailing_space.as_key(),
            "trailing whitespace must normalize to the same key"
        );
        assert_eq!(
            canonical.as_key(),
            double_space.as_key(),
            "internal double-space must collapse to the same key"
        );
    }

    // Finding #3: a state file with an unknown field must load cleanly (no corrupt fallback).
    #[test]
    fn json_forward_compat_unknown_field_loads_cleanly() {
        let dir = TempDir::new().unwrap();
        let json = r#"{"synced":["L2T|watched|tmdb:1|2024-01-01"],"future_field":"ignored"}"#;
        std::fs::write(dir.path().join("sync_state.json"), json).unwrap();

        let state = SyncState::load(dir.path());
        assert!(state.contains(&watched_l2t(1, "2024-01-01")));
        assert_eq!(state.len(), 1);
    }

    // Finding #4: mark() must be idempotent — same key twice must not inflate len().
    #[test]
    fn mark_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        state.mark(watched_l2t(42, "2024-01-01"));
        state.mark(watched_l2t(42, "2024-01-01"));

        assert_eq!(
            state.len(),
            1,
            "marking the same key twice must not double-count"
        );
    }

    // Finding #5: 1000 distinct keys survive a save+load round-trip intact.
    #[test]
    fn large_state_save_load_all_present() {
        let dir = TempDir::new().unwrap();
        let mut state = SyncState::load(dir.path());

        for i in 0u64..1000 {
            state.mark(watched_l2t(i, "2024-01-01"));
        }
        assert_eq!(state.len(), 1000);
        state.save(dir.path()).unwrap();

        let loaded = SyncState::load(dir.path());
        assert_eq!(loaded.len(), 1000);
        for i in 0u64..1000 {
            assert!(loaded.contains(&watched_l2t(i, "2024-01-01")));
        }
    }

    // Finding #6: tmdb:0 and empty title must not panic and must produce stable keys.
    #[test]
    fn tmdb_zero_and_empty_title_are_stable_no_panic() {
        let zero = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(0),
            "2024-01-01",
        );
        let key_str = zero.as_key();
        assert!(key_str.contains("tmdb:0"));
        // Second construction must give the same key.
        let zero2 = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(0),
            "2024-01-01",
        );
        assert_eq!(zero.as_key(), zero2.as_key());

        let empty_title = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear(String::new(), 1999),
            "2024-01-01",
        );
        let empty_str = empty_title.as_key();
        assert!(!empty_str.is_empty());
        // Stable across two constructions.
        let empty_title2 = SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear(String::new(), 1999),
            "2024-01-01",
        );
        assert_eq!(empty_title.as_key(), empty_title2.as_key());

        // Both keys are insertable without panicking.
        let mut state = SyncState::load(TempDir::new().unwrap().path());
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::Tmdb(0),
            "2024-01-01",
        ));
        state.mark(SyncKey::new(
            Direction::LetterboxdToTrakt,
            ItemType::Watched,
            ItemRef::TitleYear(String::new(), 1999),
            "2024-01-01",
        ));
        assert_eq!(state.len(), 2);
    }
}
