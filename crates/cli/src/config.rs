use std::io::{self, Write};

use anyhow::{Context, bail};
use debrief_summarize::config::{
    self, DEFAULT_ANSWER_TOKENS, DEFAULT_CONTEXT_TOKENS, KEYS_FILE, Keys, PROMPT_FILE, Provider,
    SETTINGS_FILE, Settings, SummarySettings, masked,
};
use debrief_summarize::{chat_models, list_models};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Set up summaries: the provider, your API key and the model.
    Summaries,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    match args.action {
        None => show(),
        Some(Action::Summaries) => set_up_summaries(),
    }
}

fn show() -> anyhow::Result<()> {
    let folder = config::folder()?;
    let all = Settings::load(&folder)?;
    println!("Recording");
    println!("  meetings folder  {}", all.library()?.display());
    let settings = all.summary;
    let keys = Keys::load(&folder)?;
    config::prompt(&folder)?;
    match settings.provider {
        None => println!("Summaries: not set up. Run `debrief config summaries`."),
        Some(provider) => {
            println!("Summaries");
            println!("  provider        {}", provider.name());
            println!("  model           {}", settings.model);
            println!("  model context   {}", tokens(settings.context_tokens));
            println!(
                "  API key         {}",
                keys.get(provider)
                    .map(masked)
                    .unwrap_or_else(|| "not saved".into())
            );
            println!(
                "  new recordings  {}",
                if settings.on_by_default {
                    "write a summary"
                } else {
                    "transcript only"
                }
            );
        }
    }
    println!("Files");
    println!("  settings  {}", folder.join(SETTINGS_FILE).display());
    println!("  prompt    {}", folder.join(PROMPT_FILE).display());
    println!("  API keys  {}", folder.join(KEYS_FILE).display());
    Ok(())
}

fn set_up_summaries() -> anyhow::Result<()> {
    let folder = config::folder()?;
    let mut settings = Settings::load(&folder)?;
    let mut keys = Keys::load(&folder)?;
    let current = settings.summary.clone();

    let provider = loop {
        let answer = ask(
            "Provider (openai or anthropic)",
            current.provider.map(Provider::name),
        )?;
        match answer.parse::<Provider>() {
            Ok(provider) => break provider,
            Err(error) => println!("{error}"),
        }
    };

    let saved = keys.get(provider).map(str::to_string);
    let question = match &saved {
        Some(key) => format!(
            "{} API key (Enter keeps {}): ",
            provider.name(),
            masked(key)
        ),
        None => format!("{} API key (the input is hidden): ", provider.name()),
    };
    let typed = rpassword::prompt_password(question).context("could not read the key")?;
    let key = match typed.trim() {
        "" => saved.context("an API key is needed")?,
        typed => typed.to_string(),
    };

    println!("Checking the key");
    let models = match list_models(provider, &key) {
        Ok(models) => models,
        Err(error) => bail!("the key didn't work: {error}"),
    };
    keys.set(provider, key);
    keys.save(&folder)?;

    let models = chat_models(provider, models);
    for (number, model) in models.iter().enumerate() {
        match model.context_tokens {
            Some(context) => println!("  {:>3}  {}  ({})", number + 1, model.id, tokens(context)),
            None => println!("  {:>3}  {}", number + 1, model.id),
        }
    }
    let default = match (current.provider, provider) {
        (Some(previous), now) if previous == now && !current.model.is_empty() => {
            Some(current.model.clone())
        }
        (_, now) if models.iter().any(|m| m.id == now.suggested_model()) => {
            Some(now.suggested_model().to_string())
        }
        _ => None,
    };
    let model = loop {
        let answer = ask("Model (number or name)", default.as_deref())?;
        let chosen = match answer.parse::<usize>() {
            Ok(number) if (1..=models.len()).contains(&number) => models[number - 1].id.clone(),
            _ => answer,
        };
        if !chosen.is_empty() {
            break chosen;
        }
    };
    let info = models.iter().find(|m| m.id == model);

    let context_tokens = match info.and_then(|m| m.context_tokens) {
        Some(context) => context,
        None => {
            let default = if current.provider == Some(provider) {
                current.context_tokens
            } else {
                DEFAULT_CONTEXT_TOKENS
            };
            loop {
                let answer = ask("Model context in tokens", Some(&default.to_string()))?;
                match answer.replace(['_', ','], "").parse::<usize>() {
                    Ok(context) if context > 0 => break context,
                    _ => println!("Type a number, for example 128000"),
                }
            }
        }
    };
    let answer_tokens = info.map_or(DEFAULT_ANSWER_TOKENS, |m| m.answer_limit());

    let default_on = if current.on_by_default { "y" } else { "n" };
    let on_by_default = ask(
        "Write summaries for new recordings? (y or n)",
        Some(default_on),
    )?
    .to_lowercase()
    .starts_with('y');

    settings.summary = SummarySettings {
        provider: Some(provider),
        model: model.clone(),
        context_tokens,
        answer_tokens,
        on_by_default,
    };
    settings.save(&folder)?;
    config::prompt(&folder)?;

    println!("Saved: summaries use {} {model}", provider.name());
    println!("  settings  {}", folder.join(SETTINGS_FILE).display());
    println!(
        "  API key   {} (only you can read it)",
        folder.join(KEYS_FILE).display()
    );
    println!(
        "  prompt    {} (edit it to change what summaries say)",
        folder.join(PROMPT_FILE).display()
    );
    Ok(())
}

fn ask(question: &str, default: Option<&str>) -> io::Result<String> {
    match default {
        Some(default) => print!("{question} [{default}]: "),
        None => print!("{question}: "),
    }
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let answer = line.trim();
    Ok(match (answer.is_empty(), default) {
        (true, Some(default)) => default.to_string(),
        _ => answer.to_string(),
    })
}

fn tokens(count: usize) -> String {
    if count >= 1_000_000 && count.is_multiple_of(1_000_000) {
        format!("{}M tokens", count / 1_000_000)
    } else if count >= 1_000 && count.is_multiple_of(1_000) {
        format!("{}k tokens", count / 1_000)
    } else {
        format!("{count} tokens")
    }
}
