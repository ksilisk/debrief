//! Debrief core: everything that happens to a meeting, without UI or platform code.

pub mod analysis;
pub mod recording;

pub const APP_NAME: &str = "Debrief";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
