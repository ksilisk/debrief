//! Whisper model files: the ones Debrief offers, where they live, and how they
//! are downloaded. A download continues where it stopped and is checked by size
//! and SHA-256 before it is used.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use debrief_core::APP_ID;
use reqwest::StatusCode;
use reqwest::header::RANGE;
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Space a download leaves free on the disk, for recordings and the system.
const SPARE_BYTES: u64 = 500_000_000;
const BUFFER_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Model {
    pub name: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const LARGE_V3_TURBO: Model = Model {
    name: "large-v3-turbo",
    file: "ggml-large-v3-turbo.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
    bytes: 1_624_555_275,
    sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
};

/// The same model with 5-bit weights: a third of the size, nearly the same quality.
pub const LARGE_V3_TURBO_Q5: Model = Model {
    name: "large-v3-turbo-q5_0",
    file: "ggml-large-v3-turbo-q5_0.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
    bytes: 574_041_195,
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
};

pub const LARGE_V3: Model = Model {
    name: "large-v3",
    file: "ggml-large-v3.bin",
    url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
    bytes: 3_095_033_483,
    sha256: "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2",
};

/// The models Debrief offers; the first is the default.
pub const MODELS: [Model; 3] = [LARGE_V3_TURBO, LARGE_V3_TURBO_Q5, LARGE_V3];
pub const DEFAULT: Model = MODELS[0];

pub fn by_name(name: &str) -> Option<Model> {
    MODELS.into_iter().find(|model| model.name == name)
}

/// The model saved in the settings, or the default when none or an unknown one is saved.
pub fn chosen(name: Option<&str>) -> Model {
    name.and_then(by_name).unwrap_or(DEFAULT)
}

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error("HOME is not set")]
    NoHome,
    #[error("could not download {url}: {source}")]
    Request {
        url: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("the download of {url} broke off: {source}")]
    Network {
        url: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("could not save {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(
        "there isn't enough free space: the download needs {} more and {} is free",
        gigabytes(*needed),
        gigabytes(*available)
    )]
    NotEnoughSpace { needed: u64, available: u64 },
    #[error("the download was stopped")]
    Cancelled,
    #[error("the downloaded {file} is damaged ({reason}); try again")]
    Damaged { file: &'static str, reason: String },
}

/// The folder the app uses too: `~/Library/Application Support/<app id>/models`.
pub fn models_folder() -> Result<PathBuf, ModelError> {
    let home = std::env::var_os("HOME").ok_or(ModelError::NoHome)?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_ID)
        .join("models"))
}

impl Model {
    pub fn path(&self) -> Result<PathBuf, ModelError> {
        Ok(models_folder()?.join(self.file))
    }

    fn partial_path(&self) -> Result<PathBuf, ModelError> {
        Ok(models_folder()?.join(format!("{}.part", self.file)))
    }

    /// Checks the size only; the full check runs after each download.
    pub fn is_downloaded(&self) -> Result<bool, ModelError> {
        Ok(fs::metadata(self.path()?).is_ok_and(|meta| meta.len() == self.bytes))
    }

    /// What an unfinished download already has; the next download continues from there.
    pub fn partial_bytes(&self) -> u64 {
        self.partial_path()
            .ok()
            .and_then(|path| fs::metadata(path).ok())
            .map_or(0, |meta| meta.len())
    }

    /// Removes the model and any unfinished download of it.
    pub fn delete(&self) -> Result<(), ModelError> {
        for path in [self.path()?, self.partial_path()?] {
            match fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(write_error(&path, error));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Downloads to a `.part` file, continuing an earlier one, checks its size
    /// and SHA-256, then moves it into place. `progress` gets the bytes done and
    /// the total. Setting `cancelled` stops it and keeps the `.part` file.
    pub fn download(
        &self,
        progress: &mut dyn FnMut(u64, u64),
        cancelled: &AtomicBool,
    ) -> Result<PathBuf, ModelError> {
        let folder = models_folder()?;
        fs::create_dir_all(&folder).map_err(|source| write_error(&folder, source))?;
        let target = folder.join(self.file);
        let partial = self.partial_path()?;

        let mut done = self.partial_bytes();
        if done > self.bytes {
            fs::remove_file(&partial).map_err(|source| write_error(&partial, source))?;
            done = 0;
        }
        if let Some(available) = free_space(&folder) {
            let needed = self.bytes - done;
            if available < needed + SPARE_BYTES {
                return Err(ModelError::NotEnoughSpace { needed, available });
            }
        }
        let mut hasher = Sha256::new();
        if done > 0 {
            hash_file(&partial, done, &mut hasher)?;
        }
        progress(done, self.bytes);

        if done < self.bytes {
            let request_error = |source| ModelError::Request {
                url: self.url,
                source,
            };
            let client = reqwest::blocking::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(None)
                .build()
                .map_err(request_error)?;
            let mut request = client.get(self.url);
            if done > 0 {
                request = request.header(RANGE, format!("bytes={done}-"));
            }
            let mut response = request
                .send()
                .and_then(|response| response.error_for_status())
                .map_err(request_error)?;
            let mut file = if done > 0 && response.status() == StatusCode::PARTIAL_CONTENT {
                OpenOptions::new().append(true).open(&partial)
            } else {
                // The server sent the whole file, so the download starts over.
                done = 0;
                hasher = Sha256::new();
                File::create(&partial)
            }
            .map_err(|source| write_error(&partial, source))?;

            let mut buffer = vec![0u8; BUFFER_BYTES];
            loop {
                if cancelled.load(Ordering::Relaxed) {
                    let _ = file.sync_all();
                    return Err(ModelError::Cancelled);
                }
                let read = response
                    .read(&mut buffer)
                    .map_err(|source| ModelError::Network {
                        url: self.url,
                        source,
                    })?;
                if read == 0 {
                    break;
                }
                file.write_all(&buffer[..read]).map_err(|source| {
                    if source.kind() == io::ErrorKind::StorageFull {
                        ModelError::NotEnoughSpace {
                            needed: self.bytes - done,
                            available: 0,
                        }
                    } else {
                        write_error(&partial, source)
                    }
                })?;
                hasher.update(&buffer[..read]);
                done += read as u64;
                progress(done, self.bytes);
            }
            file.sync_all()
                .map_err(|source| write_error(&partial, source))?;
        }

        let digest: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let problem = if done != self.bytes {
            Some(format!("{done} bytes instead of {}", self.bytes))
        } else if digest != self.sha256 {
            Some("its SHA-256 does not match".to_string())
        } else {
            None
        };
        if let Some(reason) = problem {
            let _ = fs::remove_file(&partial);
            return Err(ModelError::Damaged {
                file: self.file,
                reason,
            });
        }
        fs::rename(&partial, &target).map_err(|source| write_error(&target, source))?;
        Ok(target)
    }
}

fn hash_file(path: &Path, bytes: u64, hasher: &mut Sha256) -> Result<(), ModelError> {
    let read_error = |source| write_error(path, source);
    let mut file = File::open(path).map_err(read_error)?.take(bytes);
    let mut buffer = vec![0u8; BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(read_error)?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

/// What Finder calls available, which includes space macOS can free up itself.
#[cfg(target_os = "macos")]
fn free_space(folder: &Path) -> Option<u64> {
    use objc2_foundation::{
        NSArray, NSNumber, NSString, NSURL, NSURLVolumeAvailableCapacityForImportantUsageKey,
    };

    let url = NSURL::fileURLWithPath(&NSString::from_str(folder.to_str()?));
    let key = unsafe { NSURLVolumeAvailableCapacityForImportantUsageKey };
    let values = url
        .resourceValuesForKeys_error(&NSArray::from_slice(&[key]))
        .ok()?;
    let number = values.objectForKey(key)?.downcast::<NSNumber>().ok()?;
    u64::try_from(number.longLongValue()).ok()
}

#[cfg(not(target_os = "macos"))]
fn free_space(_folder: &Path) -> Option<u64> {
    None
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn write_error(path: &Path, source: io::Error) -> ModelError {
    ModelError::Write {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_found_by_name() {
        assert_eq!(by_name("large-v3-turbo"), Some(LARGE_V3_TURBO));
        assert_eq!(by_name("large-v3"), Some(LARGE_V3));
        assert_eq!(by_name("tiny"), None);
        assert_eq!(DEFAULT, LARGE_V3_TURBO);
    }

    #[test]
    fn every_model_has_its_own_file() {
        for (i, model) in MODELS.iter().enumerate() {
            assert!(model.url.ends_with(model.file), "{}", model.name);
            assert_eq!(model.sha256.len(), 64, "{}", model.name);
            assert!(MODELS[i + 1..].iter().all(|other| other.file != model.file));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn free_space_has_an_answer() {
        let space = free_space(&std::env::temp_dir()).expect("free space");
        assert!(space > 0);
    }

    #[test]
    fn the_space_error_reads_in_gigabytes() {
        let error = ModelError::NotEnoughSpace {
            needed: 1_624_555_275,
            available: 900_000_000,
        };
        assert_eq!(
            error.to_string(),
            "there isn't enough free space: the download needs 1.6 GB more and 0.9 GB is free"
        );
    }
}
