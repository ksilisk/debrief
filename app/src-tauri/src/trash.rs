//! Moving files and folders to the Trash, so they can be taken back.

use std::io;
use std::path::Path;

/// Through NSFileManager: going through Finder would ask for permission to control Finder.
#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};

    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("the path is not valid UTF-8"))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|error| io::Error::other(error.localizedDescription().to_string()))
}

#[cfg(not(target_os = "macos"))]
pub fn move_to_trash(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "moving to the Trash works only on macOS for now",
    ))
}
