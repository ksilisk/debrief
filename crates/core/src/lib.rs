//! Debrief core: everything that happens to a meeting, without UI or platform code.

pub mod analysis;
pub mod meeting;
pub mod recording;

pub const APP_NAME: &str = "Debrief";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod testing {
    use std::path::PathBuf;

    /// A new empty folder for one test.
    pub fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("debrief-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).expect("create temp folder");
        folder
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
