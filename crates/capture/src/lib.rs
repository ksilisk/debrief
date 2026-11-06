//! Debrief capture: recording the microphone, system audio and screen.
//!
//! Each operating system gets its own module behind one shared interface.
//! macOS uses ScreenCaptureKit; it is added in M0.2 (recording test).

/// Whether this platform has a capture module. Only macOS for now.
pub fn is_supported() -> bool {
    cfg!(target_os = "macos")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_is_supported() {
        assert_eq!(is_supported(), cfg!(target_os = "macos"));
    }
}
