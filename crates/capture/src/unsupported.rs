use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Access, CaptureError, CaptureInfo, Handler, Options, Permission, Sources};

pub fn access(_permission: Permission) -> Access {
    Access::Unknown
}

pub fn request(_permission: Permission) -> Access {
    Access::Unknown
}

pub fn sources() -> Result<Sources, CaptureError> {
    Err(CaptureError::Unsupported)
}

pub fn join_videos(_parts: &[(PathBuf, f64)], _output: &Path) -> Result<(), CaptureError> {
    Err(CaptureError::Unsupported)
}

pub struct Capture {
    info: CaptureInfo,
}

impl Capture {
    pub fn start(_options: &Options, _handler: Arc<dyn Handler>) -> Result<Self, CaptureError> {
        Err(CaptureError::Unsupported)
    }

    pub fn info(&self) -> &CaptureInfo {
        &self.info
    }

    pub fn now(&self) -> f64 {
        0.0
    }

    pub fn video_bytes(&self) -> Option<u64> {
        None
    }

    pub fn pause_video(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    pub fn resume_video(&mut self, _path: &Path) -> Result<(), CaptureError> {
        Ok(())
    }

    pub fn stop(self) -> Result<(), CaptureError> {
        Ok(())
    }
}
