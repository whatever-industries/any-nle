//! Physical media checks and guarded access to source files.

use graph::Rational;

pub mod linked_original;
pub mod rewrap_size;
pub mod source_file;

pub fn stream_duration(count: i64, time_base: Rational) -> Option<Rational> {
    Rational::new(
        i128::from(count) * i128::from(time_base.num()),
        i128::from(time_base.den()),
    )
}

/// Match the PTS, DTS, and duration of an ordered picture packet.
pub fn ordered_picture_packet(packet: [i128; 3], expected_pts: i128, frame_ticks: i128) -> bool {
    packet == [expected_pts, expected_pts, frame_ticks]
}

pub fn pcm_payload_matches(duration: i64, byte_stride: i64, size: i64) -> bool {
    duration > 0 && duration.checked_mul(byte_stride) == Some(size)
}

/// Advance a PCM cursor without crossing the declared playable end.
pub fn pcm_packet_end(cursor: i64, duration: i64, playable_end: i64) -> Option<i64> {
    if duration <= 0 {
        return None;
    }
    cursor
        .checked_add(duration)
        .filter(|&end| end <= playable_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_duration_preserves_fractional_clocks() {
        let clock = Rational::new(1001, 30000).unwrap();
        assert_eq!(stream_duration(48, clock), Rational::new(1001, 625));
        assert_eq!(stream_duration(i64::MAX, Rational::integer(2)), None);
    }

    #[test]
    fn ordered_picture_checks_both_timestamps_and_duration() {
        assert!(ordered_picture_packet([1001, 1001, 1001], 1001, 1001));
        assert!(!ordered_picture_packet([2002, 2002, 1001], 1001, 1001));
        assert!(!ordered_picture_packet([1001, 0, 1001], 1001, 1001));
        assert!(!ordered_picture_packet([1001, 1001, 1000], 1001, 1001));
    }

    #[test]
    fn pcm_payload_checks_the_full_sample_width() {
        assert!(pcm_payload_matches(1024, 6, 6144));
        assert!(!pcm_payload_matches(1024, 6, 6141));
        assert!(!pcm_payload_matches(i64::MAX, 6, 6144));
    }

    #[test]
    fn pcm_cursor_distinguishes_complete_truncated_and_overlong_extents() {
        let cursor = pcm_packet_end(0, 1024, 1050).unwrap();
        assert_eq!(pcm_packet_end(cursor, 26, 1050), Some(1050));
        assert_eq!(pcm_packet_end(cursor, 25, 1050), Some(1049));
        assert_eq!(pcm_packet_end(cursor, 27, 1050), None);
        assert_eq!(pcm_packet_end(cursor, 0, 1050), None);
        assert_eq!(pcm_packet_end(i64::MAX, 1, i64::MAX), None);
    }
}
