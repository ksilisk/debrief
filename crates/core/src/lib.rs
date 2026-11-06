//! Debrief core: everything that happens to a meeting after it is recorded.
//!
//! Planned modules (see `docs/architecture.md`):
//! - `meeting`: the meeting folder and `meeting.json` (M1.1)
//! - `pipeline`: transcription, merge by time, echo-line removal (M1.3)
//! - `export`: transcript and summary files (M1.4)
//! - `summary`: OpenAI-compatible summary client (M1.5)

/// Name shown to users.
pub const APP_NAME: &str = "Debrief";

/// Version of this build, taken from the workspace `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
