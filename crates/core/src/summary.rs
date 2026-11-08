//! Summaries: the transcript and the user's prompt go to a language model.

use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use crate::export::{details, text_for_summary, title};
use crate::files::write_atomically;
use crate::meeting::Meeting;
use crate::transcript::Transcript;

pub const MARKDOWN_FILE: &str = "summary.md";
pub const TEXT_FILE: &str = "summary.txt";

/// On the low side on purpose: Russian takes more tokens per word than English.
const CHARS_PER_TOKEN: usize = 3;

#[derive(Debug, thiserror::Error)]
pub enum SummarizeError {
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Failed(Box<dyn Error + Send + Sync>),
}

/// A language model that writes the summary.
pub trait Summarizer: Send {
    /// Saved with the step, for example `anthropic claude-opus-5-5`.
    fn model(&self) -> &str;

    /// The user's instructions.
    fn prompt(&self) -> &str;

    /// How many tokens the model reads at once.
    fn context_tokens(&self) -> usize;

    /// Tokens kept free for the answer.
    fn answer_tokens(&self) -> usize;

    /// Returns the summary as Markdown. `on_text` gets the number of characters
    /// received so far; stop when `cancelled` is set.
    fn summarize(
        &mut self,
        transcript: &str,
        on_text: &mut dyn FnMut(usize),
        cancelled: &AtomicBool,
    ) -> Result<String, SummarizeError>;
}

/// The transcript doesn't fit the model's context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "the transcript and prompt are about {needed} tokens, but the model reads at most {available} \
     (with room for the answer); pick a model with a larger context"
)]
pub struct TooLong {
    pub needed: usize,
    pub available: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("could not write {}: {source}", path.display())]
pub struct WriteError {
    path: PathBuf,
    #[source]
    source: io::Error,
}

pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(CHARS_PER_TOKEN)
}

/// A rough check before sending, so a long meeting fails at once with a clear reason.
pub fn check_fits(summarizer: &dyn Summarizer, transcript: &str) -> Result<(), TooLong> {
    let needed = estimate_tokens(summarizer.prompt()) + estimate_tokens(transcript);
    let available = summarizer
        .context_tokens()
        .saturating_sub(summarizer.answer_tokens());
    if needed > available {
        Err(TooLong { needed, available })
    } else {
        Ok(())
    }
}

/// What the model reads: the plain text transcript without echo lines.
pub fn input_text(meeting: &Meeting, transcript: &Transcript) -> String {
    text_for_summary(meeting, transcript)
}

/// Writes `summary.md` with the model's Markdown, and `summary.txt` without the
/// Markdown, both under the meeting's title and details.
pub fn write_files(
    folder: &Path,
    meeting: &Meeting,
    transcript: &Transcript,
    summary: &str,
) -> Result<(), WriteError> {
    let title = title(meeting);
    let details = details(meeting, transcript);
    let summary = summary.trim();
    for (file, content) in [
        (
            MARKDOWN_FILE,
            format!("# {title}\n\n{details}\n\n{summary}\n"),
        ),
        (
            TEXT_FILE,
            format!("{title}\n{details}\n\n{}\n", plain(summary)),
        ),
    ] {
        let path = folder.join(file);
        write_atomically(&path, content.as_bytes())
            .map_err(|source| WriteError { path, source })?;
    }
    Ok(())
}

/// Markdown made readable as plain text: no heading marks, emphasis or code ticks;
/// `*` bullets become `-`.
fn plain(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let indent = &line[..line.len() - trimmed.len()];
            let line = if let Some(heading) = trimmed.strip_prefix('#') {
                heading.trim_start_matches('#').trim_start().to_string()
            } else if let Some(item) = trimmed.strip_prefix("* ") {
                format!("{indent}- {item}")
            } else {
                line.to_string()
            };
            line.replace("**", "").replace("__", "").replace('`', "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed {
        prompt: String,
        context: usize,
    }

    impl Summarizer for Fixed {
        fn model(&self) -> &str {
            "fixed"
        }

        fn prompt(&self) -> &str {
            &self.prompt
        }

        fn context_tokens(&self) -> usize {
            self.context
        }

        fn answer_tokens(&self) -> usize {
            100
        }

        fn summarize(
            &mut self,
            _: &str,
            _: &mut dyn FnMut(usize),
            _: &AtomicBool,
        ) -> Result<String, SummarizeError> {
            Ok(String::new())
        }
    }

    #[test]
    fn a_transcript_that_fits_passes_and_a_longer_one_is_refused() {
        let summarizer = Fixed {
            prompt: "p".repeat(30),
            context: 1_100,
        };
        assert_eq!(check_fits(&summarizer, &"я".repeat(2_970)), Ok(()));
        assert_eq!(
            check_fits(&summarizer, &"я".repeat(3_000)),
            Err(TooLong {
                needed: 1_010,
                available: 1_000
            })
        );
    }

    #[test]
    fn markdown_becomes_plain_text() {
        let markdown = "## Summary\n\nThe **release** moves to `Monday`.\n\n## Action items\n\n* QA: finish tests\n  * by Friday\n- Me: tell the team";
        let expected = "Summary\n\nThe release moves to Monday.\n\nAction items\n\n- QA: finish tests\n  - by Friday\n- Me: tell the team";
        assert_eq!(plain(markdown), expected);
    }
}
