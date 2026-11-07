//! ScreenCaptureKit records the microphone and, for all apps, system audio in
//! one stream on one clock. One app's sound comes from a Core Audio process tap
//! on the same clock instead. Video, when on, is a second stream.

mod tap;

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use screencapturekit::error::SCStreamErrorCode;
use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    RecordingCallbacks, SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};
use screencapturekit::shareable_content::SCShareableContentInfo;
use screencapturekit::stream::StreamCallbacks;

use crate::{
    App, AudioChunk, CaptureError, CaptureInfo, Display, Handler, Microphone, Options, Source,
    Sources, SystemAudio, Video, VideoTarget, Window,
};
use tap::Tap;

const SAMPLE_RATE: i32 = 48_000;
const VIDEO_FINISH_TIMEOUT: Duration = Duration::from_secs(10);

pub fn sources() -> Result<Sources, CaptureError> {
    let microphones = AudioInputDevice::list()
        .into_iter()
        .map(|device| Microphone {
            id: device.id,
            name: device.name,
            is_default: device.is_default,
        })
        .collect();

    let content = SCShareableContent::get().map_err(map_error)?;

    let mut apps: Vec<App> = content
        .applications()
        .iter()
        .map(|app| App {
            name: app.application_name(),
            bundle_id: app.bundle_identifier(),
        })
        .filter(|app| !app.bundle_id.is_empty())
        .collect();
    apps.sort_by_cached_key(|app| (app.name.to_lowercase(), app.bundle_id.clone()));
    apps.dedup_by(|a, b| a.bundle_id == b.bundle_id);

    let displays = content
        .displays()
        .iter()
        .map(|display| Display {
            id: display.display_id(),
            width: display.width(),
            height: display.height(),
        })
        .collect();

    let windows = content
        .windows()
        .iter()
        .filter(|window| window.is_on_screen() && window.window_layer() == 0)
        .filter_map(|window| {
            let frame = window.frame();
            let title = window.title().filter(|title| !title.is_empty())?;
            Some(Window {
                id: window.window_id(),
                app: window
                    .owning_application()
                    .map(|app| app.application_name())
                    .unwrap_or_default(),
                title,
                width: frame.size.width as u32,
                height: frame.size.height as u32,
            })
        })
        .collect();

    Ok(Sources {
        microphones,
        apps,
        displays,
        windows,
    })
}

pub struct Capture {
    audio: SCStream,
    tap: Option<Tap>,
    video: Option<VideoCapture>,
    info: CaptureInfo,
}

struct VideoCapture {
    stream: SCStream,
    output: SCRecordingOutput,
    finished: Arc<(Mutex<bool>, Condvar)>,
}

impl Capture {
    pub fn start(options: &Options, handler: Arc<dyn Handler>) -> Result<Self, CaptureError> {
        let content = SCShareableContent::get().map_err(map_error)?;
        let displays = content.displays();
        let main_display = displays
            .first()
            .ok_or_else(|| CaptureError::NotFound("A display".into()))?;

        let microphones = AudioInputDevice::list();
        let microphone = match &options.microphone {
            Some(id) => microphones
                .iter()
                .find(|device| &device.id == id)
                .ok_or_else(|| CaptureError::NotFound(format!("Microphone {id}")))?,
            None => microphones
                .iter()
                .find(|device| device.is_default)
                .or(microphones.first())
                .ok_or_else(|| CaptureError::NotFound("A microphone".into()))?,
        };

        // Started first: creating its device can take a couple of seconds
        // (seen with AirPods), and the start of the call should not be lost.
        let (tap, system_audio) = match &options.system_audio {
            SystemAudio::AllApps => (None, "All apps".to_string()),
            SystemAudio::App { bundle_id } => (
                Some(Tap::start(bundle_id, Arc::clone(&handler))?),
                app_name(&content, bundle_id),
            ),
        };

        let filter = SCContentFilter::create()
            .with_display(main_display)
            .with_excluding_windows(&[])
            .build()
            .map_err(map_error)?;

        // The stream needs a video size even for audio only; these frames are dropped.
        let config = SCStreamConfiguration::new()
            .with_width(16)
            .with_height(16)
            .with_fps(1)
            .with_captures_audio(tap.is_none())
            .with_sample_rate(SAMPLE_RATE)
            .with_channel_count(1)
            .with_excludes_current_process_audio(true)
            .with_captures_microphone(true)
            .and_then(|config| config.with_microphone_capture_device_id(&microphone.id))
            .map_err(map_error)?;

        let on_error = Arc::clone(&handler);
        let mut audio = SCStream::new_with_delegate(
            &filter,
            &config,
            StreamCallbacks::new()
                .on_error(move |error| on_error.failed(format!("Audio stopped: {error}"))),
        )
        .map_err(map_error)?;
        audio
            .add_output_handler(|_, _| {}, SCStreamOutputType::Screen)
            .map_err(map_error)?;
        if tap.is_none() {
            let system = Arc::clone(&handler);
            audio
                .add_output_handler(
                    move |sample, _| deliver(system.as_ref(), Source::System, &sample),
                    SCStreamOutputType::Audio,
                )
                .map_err(map_error)?;
        }
        let mic = Arc::clone(&handler);
        audio
            .add_output_handler(
                move |sample, _| deliver(mic.as_ref(), Source::Microphone, &sample),
                SCStreamOutputType::Microphone,
            )
            .map_err(map_error)?;

        let (video, video_info) = match &options.video {
            Some(video) => {
                let (capture, info) = VideoCapture::new(&content, &displays, video, &handler)?;
                (Some(capture), Some(info))
            }
            None => (None, None),
        };

        audio.start_capture().map_err(map_error)?;
        if let Some(video) = &video {
            if let Err(error) = video.stream.start_capture() {
                let _ = audio.stop_capture();
                return Err(map_error(error));
            }
        }

        Ok(Self {
            audio,
            tap,
            video,
            info: CaptureInfo {
                microphone: microphone.name.clone(),
                system_audio,
                video: video_info,
            },
        })
    }

    pub fn info(&self) -> &CaptureInfo {
        &self.info
    }

    pub fn video_bytes(&self) -> Option<u64> {
        self.video
            .as_ref()
            .map(|video| u64::try_from(video.output.recorded_file_size()).unwrap_or(0))
    }

    pub fn stop(self) -> Result<(), CaptureError> {
        let mut result = Ok(());
        if let Some(video) = &self.video {
            result = video.stream.stop_capture().map_err(map_error);
            video.wait_until_finished();
        }
        let audio = self.audio.stop_capture().map_err(map_error);
        drop(self.tap);
        result.and(audio)
    }
}

fn app_name(content: &SCShareableContent, bundle_id: &str) -> String {
    content
        .applications()
        .iter()
        .find(|app| app.bundle_identifier() == bundle_id)
        .map(|app| format!("{} ({bundle_id})", app.application_name()))
        .unwrap_or_else(|| format!("{bundle_id} (not running yet)"))
}

impl VideoCapture {
    fn new(
        content: &SCShareableContent,
        displays: &[SCDisplay],
        video: &Video,
        handler: &Arc<dyn Handler>,
    ) -> Result<(Self, String), CaptureError> {
        let display_filter = |display: &SCDisplay| {
            SCContentFilter::create()
                .with_display(display)
                .with_excluding_windows(&[])
                .build()
        };
        let find_display = |id: u32| {
            displays
                .iter()
                .find(|display| display.display_id() == id)
                .ok_or_else(|| CaptureError::NotFound(format!("Display {id}")))
        };
        let (filter, target) = match video.target {
            VideoTarget::MainDisplay => {
                let display = displays
                    .first()
                    .ok_or_else(|| CaptureError::NotFound("A display".into()))?;
                (
                    display_filter(display),
                    format!("display {}", display.display_id()),
                )
            }
            VideoTarget::Display(id) => {
                (display_filter(find_display(id)?), format!("display {id}"))
            }
            VideoTarget::Window(id) => {
                let window = content
                    .windows()
                    .into_iter()
                    .find(|window| window.window_id() == id)
                    .ok_or_else(|| CaptureError::NotFound(format!("Window {id}")))?;
                let app = window
                    .owning_application()
                    .map(|app| app.application_name())
                    .unwrap_or_default();
                (
                    SCContentFilter::create().with_window(&window).build(),
                    format!("window {id} ({app})"),
                )
            }
        };
        let filter = filter.map_err(map_error)?;
        let (width, height) = SCShareableContentInfo::for_filter(&filter)
            .map(|info| info.pixel_size())
            .filter(|&(width, height)| width > 0 && height > 0)
            .ok_or_else(|| CaptureError::Platform(format!("could not get the size of {target}")))?;

        let config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_fps(video.fps)
            .with_shows_cursor(true)
            .with_captures_audio(false);

        let recording = SCRecordingOutputConfiguration::new()
            .map_err(map_error)?
            .with_output_url(&video.path)
            .map_err(|error| CaptureError::Platform(error.to_string()))?
            .with_video_codec(SCRecordingOutputCodec::HEVC)
            .with_output_file_type(SCRecordingOutputFileType::MP4);

        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let on_finish = Arc::clone(&finished);
        let on_fail = Arc::clone(&finished);
        let fail_handler = Arc::clone(handler);
        let output = SCRecordingOutput::new_with_delegate(
            &recording,
            RecordingCallbacks::new()
                .on_finish(move || mark_finished(&on_finish))
                .on_fail(move |error| {
                    fail_handler.failed(format!("Video recording failed: {error}"));
                    mark_finished(&on_fail);
                }),
        )
        .ok_or_else(|| CaptureError::Platform("could not create the video recording".into()))?;

        let on_error = Arc::clone(handler);
        let mut stream = SCStream::new_with_delegate(
            &filter,
            &config,
            StreamCallbacks::new()
                .on_error(move |error| on_error.failed(format!("Video stopped: {error}"))),
        )
        .map_err(map_error)?;
        stream
            .add_output_handler(|_, _| {}, SCStreamOutputType::Screen)
            .map_err(map_error)?;
        stream.add_recording_output(&output).map_err(map_error)?;

        let info = format!("{target}, {width} × {height}, {} fps", video.fps);
        Ok((
            Self {
                stream,
                output,
                finished,
            },
            info,
        ))
    }

    fn wait_until_finished(&self) {
        let (lock, condvar) = &*self.finished;
        let done = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = condvar
            .wait_timeout_while(done, VIDEO_FINISH_TIMEOUT, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

fn mark_finished(finished: &(Mutex<bool>, Condvar)) {
    let (lock, condvar) = finished;
    *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
    condvar.notify_all();
}

fn deliver(handler: &dyn Handler, source: Source, sample: &CMSampleBuffer) {
    let Some(time) = sample.presentation_timestamp().as_seconds() else {
        return;
    };
    if let Some((sample_rate, samples)) = mono_samples(sample) {
        handler.audio(AudioChunk {
            source,
            time,
            sample_rate,
            samples: &samples,
        });
    }
}

fn mono_samples(sample: &CMSampleBuffer) -> Option<(u32, Vec<f32>)> {
    let format = sample.format_description()?;
    let sample_rate = format.audio_sample_rate()?.round() as u32;
    let bits = format.audio_bits_per_channel()?;
    let is_float = format.audio_is_float();
    let frames = usize::try_from(sample.num_samples()).ok()?;
    let buffers = sample.audio_buffer_list().ok()?;

    let mut mono = vec![0.0f32; frames];
    let mut channels = 0;
    for buffer in buffers.iter() {
        let count = buffer.number_channels as usize;
        if count == 0 {
            continue;
        }
        let values = decode(buffer.data(), bits, is_float)?;
        for (frame, values) in mono.iter_mut().zip(values.chunks_exact(count)) {
            *frame += values.iter().sum::<f32>();
        }
        channels += count;
    }
    if channels == 0 {
        return None;
    }
    let scale = 1.0 / channels as f32;
    mono.iter_mut().for_each(|value| *value *= scale);
    Some((sample_rate, mono))
}

fn decode(bytes: &[u8], bits: u32, is_float: bool) -> Option<Vec<f32>> {
    match (is_float, bits) {
        (true, 32) => Some(
            bytes
                .chunks_exact(4)
                .map(|b| f32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
                .collect(),
        ),
        (false, 16) => Some(
            bytes
                .chunks_exact(2)
                .map(|b| f32::from(i16::from_ne_bytes([b[0], b[1]])) / 32_768.0)
                .collect(),
        ),
        (false, 32) => Some(
            bytes
                .chunks_exact(4)
                .map(|b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0)
                .collect(),
        ),
        _ => None,
    }
}

fn map_error(error: SCError) -> CaptureError {
    match &error {
        SCError::NoShareableContent(_) | SCError::PermissionDenied(_) => {
            CaptureError::Permission(error.to_string())
        }
        _ if error.stream_error_code() == Some(SCStreamErrorCode::UserDeclined) => {
            CaptureError::Permission(error.to_string())
        }
        _ => CaptureError::Platform(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_float_and_integer_samples() {
        let float: Vec<u8> = [0.5f32, -0.25]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        assert_eq!(decode(&float, 32, true), Some(vec![0.5, -0.25]));
        let int: Vec<u8> = [16_384i16, -32_768]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect();
        assert_eq!(decode(&int, 16, false), Some(vec![0.5, -1.0]));
        assert_eq!(decode(&int, 24, false), None);
    }
}
