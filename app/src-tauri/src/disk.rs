//! Free space on the disk with the meetings folder, so a recording stops before
//! the disk is full.

use std::path::Path;

use debrief_summarize::config;
use debrief_transcribe::models::free_space;
use serde::Serialize;

/// A recording stops, or doesn't start, with less than this free.
pub const STOP_BELOW: u64 = 500_000_000;
const LOW_BELOW: u64 = 2_000_000_000;
const LOW_MINUTES: u64 = 15;
/// The first seconds of a recording don't tell its rate yet.
const RATE_AFTER_SECONDS: f64 = 30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub free_bytes: u64,
    /// Recording time left at the current rate before the recording stops;
    /// `None` until the rate is known.
    pub minutes_left: Option<u64>,
    pub low: bool,
}

/// Where new meetings go.
pub fn for_new_recording() -> Option<Space> {
    let free = free(&config::library().ok()?)?;
    Some(space(free, None))
}

/// The recording in `folder` wrote `bytes` in `seconds`.
pub fn while_recording(folder: &Path, seconds: f64, bytes: u64) -> Option<Space> {
    let free = free(folder)?;
    let minutes_left = (seconds >= RATE_AFTER_SECONDS && bytes > 0).then(|| {
        let per_second = bytes as f64 / seconds;
        (free.saturating_sub(STOP_BELOW) as f64 / per_second / 60.0) as u64
    });
    Some(space(free, minutes_left))
}

/// For example "1.4 GB" or "54 GB".
pub fn gigabytes(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb < 10.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{gb:.0} GB")
    }
}

fn space(free_bytes: u64, minutes_left: Option<u64>) -> Space {
    Space {
        free_bytes,
        minutes_left,
        low: free_bytes < LOW_BELOW || minutes_left.is_some_and(|minutes| minutes < LOW_MINUTES),
    }
}

/// The meetings folder may not exist yet; its disk is the nearest folder's that does.
fn free(folder: &Path) -> Option<u64> {
    free_space(folder.ancestors().find(|path| path.exists())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_by_space_or_by_time_left() {
        assert!(!space(50_000_000_000, None).low);
        assert!(space(1_500_000_000, None).low);
        assert!(space(50_000_000_000, Some(10)).low);
        assert!(!space(50_000_000_000, Some(90)).low);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn time_left_follows_the_rate_and_keeps_the_stop_margin() {
        let folder = std::env::temp_dir();
        let space = while_recording(&folder, 60.0, 60 * 1_000_000).expect("space");
        let expected = (space.free_bytes - STOP_BELOW) / 1_000_000 / 60;
        assert!(space.minutes_left.expect("minutes").abs_diff(expected) <= 1);
        assert_eq!(
            while_recording(&folder, 10.0, 1_000)
                .expect("space")
                .minutes_left,
            None
        );
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(gigabytes(1_449_000_000), "1.4 GB");
        assert_eq!(gigabytes(54_200_000_000), "54 GB");
    }
}
