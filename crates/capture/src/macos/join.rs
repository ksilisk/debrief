//! Joins video files with AVFoundation, copying the samples without re-encoding.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_av_foundation::{
    AVAssetExportPresetPassthrough, AVAssetExportSession, AVAssetExportSessionStatus, AVAssetTrack,
    AVFileTypeMPEG4, AVMediaTypeVideo, AVMutableComposition, AVURLAsset,
};
use objc2_core_media::{CMTime, CMTimeRange, kCMPersistentTrackID_Invalid, kCMTimeZero};
use objc2_foundation::{NSError, NSString, NSURL};

use crate::CaptureError;

const TIMESCALE: i32 = 600;
const EXPORT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub fn join_videos(parts: &[(PathBuf, f64)], output: &Path) -> Result<(), CaptureError> {
    let video = unsafe { AVMediaTypeVideo }.ok_or_else(|| failed("no video media type"))?;
    let composition = unsafe { AVMutableComposition::composition() };
    let track = unsafe {
        composition
            .addMutableTrackWithMediaType_preferredTrackID(video, kCMPersistentTrackID_Invalid)
    }
    .ok_or_else(|| failed("could not create the video track"))?;

    // A track does not keep its asset alive, and inserting a track whose asset
    // is gone fails (-12780), so the assets live until the export is done.
    let mut assets = Vec::with_capacity(parts.len());
    let mut end = unsafe { kCMTimeZero };
    for (path, at) in parts {
        let (asset, source) = video_track(path)?;
        assets.push(asset);
        let range = unsafe { source.timeRange() };
        let start = unsafe { CMTime::with_seconds(*at, TIMESCALE) };
        if unsafe { start.compare(end) } > 0 {
            let gap = CMTimeRange {
                start: end,
                duration: unsafe { start.subtract(end) },
            };
            unsafe { track.insertEmptyTimeRange(gap) };
            end = start;
        }
        unsafe { track.insertTimeRange_ofTrack_atTime_error(range, &source, end) }
            .map_err(|error| failed(&describe(&error)))?;
        end = unsafe { end.add(range.duration) };
    }

    let session = unsafe {
        AVAssetExportSession::exportSessionWithAsset_presetName(
            &composition,
            AVAssetExportPresetPassthrough,
        )
    }
    .ok_or_else(|| failed("could not start the export"))?;
    let file_type = unsafe { AVFileTypeMPEG4 }.ok_or_else(|| failed("no MP4 file type"))?;
    unsafe {
        session.setOutputURL(Some(&file_url(output)));
        session.setOutputFileType(Some(file_type));
    }
    let (done, finished) = mpsc::channel();
    let handler = RcBlock::new(move || {
        let _ = done.send(());
    });
    unsafe { session.exportAsynchronouslyWithCompletionHandler(&handler) };
    finished
        .recv_timeout(EXPORT_TIMEOUT)
        .map_err(|_| failed("the export did not finish"))?;
    drop(assets);

    if unsafe { session.status() } == AVAssetExportSessionStatus::Completed {
        Ok(())
    } else {
        let reason = unsafe { session.error() }
            .map(|error| describe(&error))
            .unwrap_or_else(|| "the export failed".into());
        Err(failed(&reason))
    }
}

fn video_track(
    path: &Path,
) -> Result<(Retained<AVURLAsset>, Retained<AVAssetTrack>), CaptureError> {
    let video = unsafe { AVMediaTypeVideo }.ok_or_else(|| failed("no video media type"))?;
    let asset = unsafe { AVURLAsset::URLAssetWithURL_options(&file_url(path), None) };
    // The async loader calls back on another thread, and the track can't be sent
    // across; this call loads the tracks here instead.
    #[allow(deprecated)]
    let tracks = unsafe { asset.tracksWithMediaType(video) };
    let track = tracks
        .firstObject()
        .ok_or_else(|| failed(&format!("{} has no video", path.display())))?;
    Ok((asset, track))
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

fn describe(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

fn failed(reason: &str) -> CaptureError {
    CaptureError::Platform(format!("joining the video failed: {reason}"))
}
