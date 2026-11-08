use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use debrief_core::meeting::Language;
use debrief_core::process::{Progress, TranscribeError, Transcriber};
use debrief_core::transcript::{Line, TrackText};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MAX_THREADS: usize = 8;

#[derive(Debug, thiserror::Error)]
#[error("could not load the Whisper model {}: {source}", path.display())]
pub struct LoadError {
    path: PathBuf,
    #[source]
    source: whisper_rs::WhisperError,
}

/// Whisper on the GPU (Metal on macOS). The model stays loaded for both tracks.
pub struct Whisper {
    context: WhisperContext,
    model: String,
    threads: i32,
}

impl Whisper {
    pub fn load(path: &Path, model: &str) -> Result<Self, LoadError> {
        // whisper.cpp logs to stderr by default; this sends its logs nowhere.
        whisper_rs::install_logging_hooks();
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true).flash_attn(true);
        let context =
            WhisperContext::new_with_params(path, params).map_err(|source| LoadError {
                path: path.to_path_buf(),
                source,
            })?;
        let threads =
            std::thread::available_parallelism().map_or(4, |n| n.get().min(MAX_THREADS)) as i32;
        Ok(Self {
            context,
            model: model.to_string(),
            threads,
        })
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
        let failed = |error: whisper_rs::WhisperError| TranscribeError::Failed(Box::new(error));
        let mut state = self.context.create_state().map_err(failed)?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 5 });
        params.set_language(Some(language.code()));
        params.set_n_threads(self.threads);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        let reporter = progress.clone();
        params.set_progress_callback_safe(move |percent: i32| {
            reporter.set(percent as f32 / 100.0);
        });
        // whisper-rs 0.16's `set_abort_callback_safe` casts its boxed closure to the
        // wrong type, so whisper.cpp aborts at once; this reads the flag directly.
        let cancelled = progress.cancelled_flag();
        unsafe {
            params.set_abort_callback(Some(should_abort));
            params.set_abort_callback_user_data(Arc::as_ptr(&cancelled) as *mut c_void);
        }

        let result = state.full(params, audio);
        drop(cancelled);
        if progress.is_cancelled() {
            return Err(TranscribeError::Cancelled);
        }
        result.map_err(failed)?;

        let detected = whisper_rs::get_lang_str(state.full_lang_id_from_state())
            .unwrap_or(language.code())
            .to_string();
        let lines = state
            .as_iter()
            .filter_map(|segment| {
                let text = segment.to_str_lossy().ok()?.trim().to_string();
                (!text.is_empty()).then(|| Line {
                    start: segment.start_timestamp() as f64 / 100.0,
                    end: segment.end_timestamp() as f64 / 100.0,
                    text,
                })
            })
            .collect();
        Ok(TrackText {
            language: detected,
            lines,
        })
    }
}

/// SAFETY: `data` points to the `AtomicBool` that `transcribe` keeps alive until
/// `full` returns.
unsafe extern "C" fn should_abort(data: *mut c_void) -> bool {
    let cancelled = unsafe { &*(data as *const AtomicBool) };
    cancelled.load(Ordering::Relaxed)
}
