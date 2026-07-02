#![allow(dead_code)]

/// Converts a Trakt integer rating (1–10) to a Letterboxd half-star rating (0.5–5.0).
pub fn trakt_rating_to_letterboxd(trakt: u8) -> f32 {
    trakt as f32 / 2.0
}

/// Converts a Letterboxd half-star rating (0.5–5.0) to a Trakt integer rating (1–10).
/// Multiplies by 2, rounds to nearest integer, and clamps to 1..=10.
pub fn letterboxd_rating_to_trakt(letterboxd: f32) -> u8 {
    let raw = (letterboxd * 2.0).round() as i32;
    raw.clamp(1, 10) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trakt_to_letterboxd_boundaries() {
        assert_eq!(trakt_rating_to_letterboxd(1), 0.5);
        assert_eq!(trakt_rating_to_letterboxd(7), 3.5);
        assert_eq!(trakt_rating_to_letterboxd(10), 5.0);
    }

    #[test]
    fn letterboxd_to_trakt_boundaries() {
        assert_eq!(letterboxd_rating_to_trakt(0.5), 1);
        assert_eq!(letterboxd_rating_to_trakt(3.5), 7);
        assert_eq!(letterboxd_rating_to_trakt(5.0), 10);
    }

    #[test]
    fn letterboxd_to_trakt_clamps_low() {
        assert_eq!(letterboxd_rating_to_trakt(0.1), 1);
    }

    #[test]
    fn letterboxd_to_trakt_clamps_high() {
        assert_eq!(letterboxd_rating_to_trakt(5.5), 10);
    }

    #[test]
    fn round_trip_all_half_stars() {
        for trakt in 1u8..=10 {
            let lb = trakt_rating_to_letterboxd(trakt);
            assert_eq!(letterboxd_rating_to_trakt(lb), trakt);
        }
    }

    // ── Gap coverage (FG-7 verify) ────────────────────────────────────────────

    #[test]
    fn even_trakt_value_round_trip_is_stable() {
        // Even Trakt values map to whole-star Letterboxd values: 8 → 4.0 → 8.
        assert_eq!(trakt_rating_to_letterboxd(8), 4.0_f32);
        assert_eq!(letterboxd_rating_to_trakt(4.0), 8);
    }

    #[test]
    fn odd_trakt_value_round_trip_is_stable() {
        // Odd Trakt values map to half-star Letterboxd values: 7 → 3.5 → 7.
        assert_eq!(trakt_rating_to_letterboxd(7), 3.5_f32);
        assert_eq!(letterboxd_rating_to_trakt(3.5), 7);
    }

    #[test]
    fn non_half_star_letterboxd_to_trakt_is_lossy() {
        // Letterboxd → Trakt → Letterboxd is lossy when the input is not a
        // half-star multiple. 2.3 × 2 = 4.6 → rounds to 5 (Trakt) → 2.5 ≠ 2.3.
        // The lossless direction is always Trakt→LB→Trakt (proven by round_trip_all_half_stars).
        let lb_in = 2.3_f32;
        let trakt = letterboxd_rating_to_trakt(lb_in);
        let lb_out = trakt_rating_to_letterboxd(trakt);
        assert_eq!(trakt, 5);
        assert!((lb_out - 2.5_f32).abs() < f32::EPSILON);
        assert!(
            (lb_out - lb_in).abs() > 0.01,
            "round-trip must be lossy for non-half-star inputs"
        );
    }
}
