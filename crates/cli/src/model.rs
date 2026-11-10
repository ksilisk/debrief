use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, bail};
use debrief_summarize::config::{self, Settings};
use debrief_transcribe::models::{self, MODELS, Model, ModelError};

use crate::format::size;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Download a Whisper model into the app's models folder. Default: the one
    /// Settings uses. An interrupted download continues where it stopped.
    Download {
        /// For example large-v3-turbo-q5_0. `debrief model` lists them.
        name: Option<String>,
    },
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let chosen = chosen()?;
    match args.action {
        None => {
            for model in MODELS {
                let state = if model.is_downloaded()? {
                    "downloaded".to_string()
                } else if model.partial_bytes() > 0 {
                    format!("{} downloaded", size(model.partial_bytes()))
                } else {
                    "not downloaded".to_string()
                };
                let mark = if model == chosen { "*" } else { " " };
                println!("{mark} {:20} {:>8}  {state}", model.name, size(model.bytes));
            }
            println!("* transcribes meetings. Download one with `debrief model download <name>`.");
            Ok(())
        }
        Some(Action::Download { name }) => {
            let model = match name {
                Some(name) => models::by_name(&name).with_context(|| {
                    format!("there is no model {name}; `debrief model` lists them")
                })?,
                None => chosen,
            };
            download(model)
        }
    }
}

fn chosen() -> anyhow::Result<Model> {
    let settings = Settings::load(&config::folder()?)?;
    Ok(models::chosen(settings.transcription.model.as_deref()))
}

fn download(model: Model) -> anyhow::Result<()> {
    if model.is_downloaded()? {
        println!("Whisper {} is already downloaded.", model.name);
        return Ok(());
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let on_ctrl_c = Arc::clone(&cancelled);
    ctrlc::set_handler(move || on_ctrl_c.store(true, Ordering::SeqCst))
        .context("could not catch Ctrl+C")?;

    println!("Downloading Whisper {} ({})", model.name, size(model.bytes));
    let started = Instant::now();
    let start = model.partial_bytes();
    let result = model.download(
        &mut |done, total| {
            let seconds = started.elapsed().as_secs_f64().max(0.1);
            print!(
                "\r  {:5.1}%  {} of {}  {}/s   ",
                done as f64 * 100.0 / total as f64,
                size(done),
                size(total),
                size((done.saturating_sub(start) as f64 / seconds) as u64),
            );
            let _ = io::stdout().flush();
        },
        &cancelled,
    );
    println!();
    match result {
        Ok(path) => {
            println!("Saved to {}", path.display());
            Ok(())
        }
        Err(ModelError::Cancelled) => {
            bail!("stopped; run the same command again to continue")
        }
        Err(error) => Err(error.into()),
    }
}
