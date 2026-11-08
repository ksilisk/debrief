//! Whisper model files: where they live and how they are downloaded.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

const APP_ID: &str = "io.github.ksilisk.debrief";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// Checks the size only; the full check runs after each download.
    pub fn is_downloaded(&self) -> Result<bool, ModelError> {
        Ok(fs::metadata(self.path()?).is_ok_and(|meta| meta.len() == self.bytes))
    }

    /// Downloads to a `.part` file, checks its size and SHA-256, then moves it
    /// into place. `progress` gets the bytes done and the total.
    pub fn download(&self, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, ModelError> {
        let folder = models_folder()?;
        fs::create_dir_all(&folder).map_err(|source| write_error(&folder, source))?;
        let target = folder.join(self.file);
        let partial = folder.join(format!("{}.part", self.file));

        let request_error = |source| ModelError::Request {
            url: self.url,
            source,
        };
        let mut response = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(None)
            .build()
            .map_err(request_error)?
            .get(self.url)
            .send()
            .and_then(|response| response.error_for_status())
            .map_err(request_error)?;

        let mut file = File::create(&partial).map_err(|source| write_error(&partial, source))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let read = response
                .read(&mut buffer)
                .map_err(|source| ModelError::Network {
                    url: self.url,
                    source,
                })?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])
                .map_err(|source| write_error(&partial, source))?;
            hasher.update(&buffer[..read]);
            done += read as u64;
            progress(done, self.bytes);
        }
        file.sync_all()
            .map_err(|source| write_error(&partial, source))?;
        drop(file);

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

fn write_error(path: &Path, source: io::Error) -> ModelError {
    ModelError::Write {
        path: path.to_path_buf(),
        source,
    }
}
