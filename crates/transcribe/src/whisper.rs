use std::ffi::{CStr, CString, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use debrief_core::meeting::Language;
use debrief_core::process::{Progress, TranscribeError, Transcriber};
use debrief_core::transcript::{Line, TrackText};
use whisper_rs::whisper_rs_sys as sys;

use crate::invented::without_invented;
use crate::speech::{Joined, Span, Vad, snap};

const MAX_THREADS: usize = 8;
const BEST_OF: c_int = 5;
/// Whisper reads its own latest text before each window. With the default
/// (about 224 tokens) a quick back-and-forth on one track fills it with short
/// replies, Whisper starts repeating them, and whole minutes come out empty.
const CARRIED_TOKENS: c_int = 128;

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("could not load the Whisper model {}", path.display())]
    Model { path: PathBuf },
    #[error("could not load the speech detector (Silero VAD)")]
    Vad,
}

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error("there is no audio to transcribe")]
    NoAudio,
    #[error("the speech detector (Silero VAD) could not read the audio")]
    NoSpeechCheck,
    #[error("Whisper could not set up its working memory")]
    NoState,
    #[error("Whisper stopped with error {0}")]
    Failed(c_int),
}

/// Whisper on the GPU (Metal on macOS). The model stays loaded for both tracks.
///
/// Whisper only gets the speech that Silero VAD finds: on silence, which is
/// most of each track, it writes lines that were never said.
///
/// It calls whisper.cpp's C API directly: whisper-rs has no wrapper for the
/// speech detector.
pub struct Whisper {
    context: NonNull<sys::whisper_context>,
    vad: Vad,
    model: String,
    threads: c_int,
}

// SAFETY: a whisper.cpp context can move to another thread; `&mut self` keeps
// it to one thread at a time.
unsafe impl Send for Whisper {}

impl Whisper {
    pub fn load(path: &Path, model: &str) -> Result<Self, LoadError> {
        let failed = || LoadError::Model {
            path: path.to_path_buf(),
        };
        // whisper.cpp logs to stderr by default; this sends its logs nowhere.
        whisper_rs::install_logging_hooks();
        let c_path = path
            .to_str()
            .and_then(|path| CString::new(path).ok())
            .ok_or_else(failed)?;
        let context = unsafe {
            let mut params = sys::whisper_context_default_params();
            params.use_gpu = true;
            params.flash_attn = true;
            sys::whisper_init_from_file_with_params_no_state(c_path.as_ptr(), params)
        };
        let context = NonNull::new(context).ok_or_else(failed)?;
        let vad = match Vad::load() {
            Some(vad) => vad,
            None => {
                unsafe { sys::whisper_free(context.as_ptr()) };
                return Err(LoadError::Vad);
            }
        };
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().min(MAX_THREADS)) as c_int;
        Ok(Self {
            context,
            vad,
            model: model.to_string(),
            threads,
        })
    }
}

impl Drop for Whisper {
    fn drop(&mut self) {
        unsafe { sys::whisper_free(self.context.as_ptr()) }
    }
}

impl Transcriber for Whisper {
    fn model(&self) -> &str {
        &self.model
    }

    fn transcribe(
        &mut self,
        audio: &[f32],
        language: Language,
        progress: &Progress,
    ) -> Result<TrackText, TranscribeError> {
        let failed = |error: RunError| TranscribeError::Failed(Box::new(error));
        if audio.is_empty() {
            return Err(failed(RunError::NoAudio));
        }
        let speech = self
            .vad
            .speech(audio)
            .ok_or_else(|| failed(RunError::NoSpeechCheck))?;
        if progress.is_cancelled() {
            return Err(TranscribeError::Cancelled);
        }
        if speech.is_empty() {
            return Ok(TrackText {
                language: match language {
                    Language::Auto => String::new(),
                    fixed => fixed.code().to_string(),
                },
                lines: Vec::new(),
            });
        }
        let joined = Joined::new(audio, &speech);
        let state = State::new(self.context).ok_or_else(|| failed(RunError::NoState))?;

        let mut params = unsafe {
            sys::whisper_full_default_params(sys::whisper_sampling_strategy_WHISPER_SAMPLING_GREEDY)
        };
        params.greedy.best_of = BEST_OF;
        params.n_max_text_ctx = CARRIED_TOKENS;
        params.language = language_code(language).as_ptr();
        params.n_threads = self.threads;
        params.print_progress = false;
        params.print_realtime = false;
        params.print_special = false;
        params.print_timestamps = false;
        // Otherwise large-v3 can fill a track with "*звук текста*" and the like.
        params.suppress_nst = true;
        params.progress_callback = Some(on_progress);
        params.progress_callback_user_data = progress as *const Progress as *mut c_void;
        let cancelled = progress.cancelled_flag();
        params.abort_callback = Some(should_abort);
        params.abort_callback_user_data = Arc::as_ptr(&cancelled) as *mut c_void;

        let code = unsafe {
            sys::whisper_full_with_state(
                self.context.as_ptr(),
                state.0.as_ptr(),
                params,
                joined.audio.as_ptr(),
                joined.audio.len() as c_int,
            )
        };
        drop(cancelled);
        if progress.is_cancelled() {
            return Err(TranscribeError::Cancelled);
        }
        if code != 0 {
            return Err(failed(RunError::Failed(code)));
        }
        Ok(state.text(language, &joined, &speech))
    }
}

/// One transcription's working memory, freed when dropped.
struct State(NonNull<sys::whisper_state>);

impl State {
    fn new(context: NonNull<sys::whisper_context>) -> Option<Self> {
        NonNull::new(unsafe { sys::whisper_init_state(context.as_ptr()) }).map(Self)
    }

    /// Lines with their times in the track, without the ones Whisper invented.
    fn text(&self, language: Language, joined: &Joined, speech: &[Span]) -> TrackText {
        let state = self.0.as_ptr();
        let detected = unsafe {
            let code = sys::whisper_lang_str(sys::whisper_full_lang_id_from_state(state));
            (!code.is_null()).then(|| CStr::from_ptr(code).to_string_lossy().into_owned())
        };
        let count = unsafe { sys::whisper_full_n_segments_from_state(state) };
        let lines = (0..count)
            .filter_map(|segment| unsafe {
                let text = sys::whisper_full_get_segment_text_from_state(state, segment);
                if text.is_null() {
                    return None;
                }
                let text = CStr::from_ptr(text).to_string_lossy().trim().to_string();
                if text.is_empty() {
                    return None;
                }
                let start = sys::whisper_full_get_segment_t0_from_state(state, segment) as f64;
                let end = sys::whisper_full_get_segment_t1_from_state(state, segment) as f64;
                let (start, end) = snap(
                    joined.in_track(start / 100.0, true),
                    joined.in_track(end / 100.0, false),
                    speech,
                );
                Some(Line {
                    start,
                    end: end.max(start),
                    text,
                })
            })
            .collect();
        TrackText {
            language: detected.unwrap_or_else(|| language.code().to_string()),
            lines: without_invented(lines),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        unsafe { sys::whisper_free_state(self.0.as_ptr()) }
    }
}

fn language_code(language: Language) -> &'static CStr {
    match language {
        Language::Auto => c"auto",
        Language::Russian => c"ru",
        Language::English => c"en",
    }
}

/// SAFETY: `data` points to the `Progress` that `transcribe` borrows until
/// `whisper_full_with_state` returns.
unsafe extern "C" fn on_progress(
    _context: *mut sys::whisper_context,
    _state: *mut sys::whisper_state,
    percent: c_int,
    data: *mut c_void,
) {
    let progress = unsafe { &*(data as *const Progress) };
    progress.set(percent as f32 / 100.0);
}

/// SAFETY: `data` points to the `AtomicBool` that `transcribe` keeps alive until
/// `whisper_full_with_state` returns.
unsafe extern "C" fn should_abort(data: *mut c_void) -> bool {
    let cancelled = unsafe { &*(data as *const AtomicBool) };
    cancelled.load(Ordering::Relaxed)
}
