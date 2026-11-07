use std::sync::Arc;

use crate::{CaptureError, CaptureInfo, Handler, Options, Sources};

pub fn sources() -> Result<Sources, CaptureError> {
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

    pub fn video_bytes(&self) -> Option<u64> {
        None
    }

    pub fn stop(self) -> Result<(), CaptureError> {
        Ok(())
    }
}
