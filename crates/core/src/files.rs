use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

/// Writes `.name.tmp` next to `path` and renames it over `path`, so a crash
/// never leaves half a file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("no file name"))?
        .to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let mut file = File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)
}
