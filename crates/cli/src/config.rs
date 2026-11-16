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
    /// Set up summaries: the provider or an OpenAI-compatible server, the API key and the model.
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
    match settings.endpoint() {
        None => println!("Summaries: not set up. Run `debrief config summaries`."),
        Some(endpoint) => {
            println!("Summaries");
            match endpoint.provider {
                Provider::Server => {
                    println!("  server          {} ({})", endpoint.name, endpoint.url)
                }
                provider => println!("  provider        {}", provider.name()),
            }
            println!("  model           {}", settings.model);
            println!("  model context   {}", tokens(settings.context_tokens));
            let none = if endpoint.needs_key() {
                "not saved"
            } else {
                "none"
            };
            println!(
                "  API key         {}",
                keys.get(&endpoint.key_name)
                    .map(masked)
                    .unwrap_or_else(|| none.into())
            );
        }
    }
    if !settings.servers.is_empty() {
        println!("Servers");
        for server in &settings.servers {
            println!("  {:>3}  {}  {}", server.id, server.name(), server.url);
        }
    }
    println!("Files");
    println!("  settings  {}", folder.join(SETTINGS_FILE).display());
    println!("  prompt    {}", folder.join(PROMPT_FILE).display());
    println!("  API keys  {}", folder.join(KEYS_FILE).display());
    Ok(())
}

/// A server from the list, or a new one; `None` for the id of a new one.
struct ServerChoice {
    id: Option<u32>,
    label: String,
    url: String,
}

fn ask_server(summary: &SummarySettings) -> anyhow::Result<ServerChoice> {
    for server in &summary.servers {
        println!("  {:>3}  {}  {}", server.id, server.name(), server.url);
    }
    let current = summary
        .server
        .filter(|_| summary.provider == Some(Provider::Server))
        .map(|id| id.to_string());
    let question = if summary.servers.is_empty() {
        "Server address, such as http://localhost:11434/v1"
    } else {
        "Server (number, or the address of a new one)"
    };
    loop {
        let answer = ask(question, current.as_deref())?;
        if let Some(server) = answer
            .parse::<u32>()
            .ok()
            .and_then(|id| summary.find_server(id))
        {
            return Ok(ServerChoice {
                id: Some(server.id),
                label: server.label.clone(),
                url: server.url.clone(),
            });
        }
        match config::server_url(&answer) {
            Ok(url) => {
                let label = ask("Name (Enter shows the address)", None)?;
                return Ok(ServerChoice {
                    id: None,
                    label,
                    url,
                });
            }
            Err(error) => println!("{error}"),
        }
    }
}

fn set_up_summaries() -> anyhow::Result<()> {
    let folder = config::folder()?;
    let mut settings = Settings::load(&folder)?;
    let mut keys = Keys::load(&folder)?;
    let current = settings.summary.clone();

    let provider = loop {
        let answer = ask(
            "Provider (openai, anthropic, openrouter or server)",
            current.provider.map(Provider::name),
        )?;
        match answer.parse::<Provider>() {
            Ok(provider) => break provider,
            Err(error) => println!("{error}"),
        }
    };
    let server = (provider == Provider::Server)
        .then(|| ask_server(&current))
        .transpose()?;
    let url = match (&server, provider.url()) {
        (Some(server), _) => server.url.clone(),
        (None, Some(url)) => url.to_string(),
        (None, None) => bail!("no address for {}", provider.name()),
    };

    // A server's saved key goes only to the address it was saved for.
    let saved = match &server {
        Some(ServerChoice { id: Some(id), .. }) => keys.get(&config::server_key_name(*id)),
        Some(_) => None,
        None => keys.get(provider.name()),
    }
    .map(str::to_string);
    let question = match (&saved, &server) {
        (Some(key), _) => format!("API key (Enter keeps {}): ", masked(key)),
        (None, Some(_)) => "API key, if the server needs one (the input is hidden): ".to_string(),
        (None, None) => format!("{} API key (the input is hidden): ", provider.name()),
    };
    let typed = rpassword::prompt_password(question).context("could not read the key")?;
    let key = match typed.trim() {
        "" if server.is_some() => saved,
        "" => Some(saved.context("an API key is needed")?),
        typed => Some(typed.to_string()),
    };

    println!("Checking the connection");
    let models = match list_models(provider, &url, key.as_deref()) {
        Ok(models) => models,
        Err(error) => bail!("the connection didn't work: {error}"),
    };
    let key_name = match server {
        Some(server) => {
            let id = settings
                .summary
                .put_server(server.id, &server.label, &server.url);
            settings.summary.server = Some(id);
            config::server_key_name(id)
        }
        None => {
            settings.summary.server = None;
            provider.name().to_string()
        }
    };
    if let Some(key) = key {
        keys.set(&key_name, key);
        keys.save(&folder)?;
    }

    let models = chat_models(provider, models);
    for (number, model) in models.iter().enumerate() {
        match model.context_tokens {
            Some(context) => println!("  {:>3}  {}  ({})", number + 1, model.id, tokens(context)),
            None => println!("  {:>3}  {}", number + 1, model.id),
        }
    }
    let same = current.provider == Some(provider) && current.server == settings.summary.server;
    let suggested = provider
        .suggested_model()
        .filter(|id| models.iter().any(|m| m.id == *id))
        .map(str::to_string);
    let default = if same && !current.model.is_empty() {
        Some(current.model.clone())
    } else if models.len() == 1 {
        Some(models[0].id.clone())
    } else {
        suggested
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
            let default = if same {
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

    let summary = &mut settings.summary;
    summary.provider = Some(provider);
    summary.model = model.clone();
    summary.context_tokens = context_tokens;
    summary.answer_tokens = answer_tokens;
    let name = summary
        .endpoint()
        .map_or_else(|| provider.name().to_string(), |endpoint| endpoint.name);
    settings.save(&folder)?;
    config::prompt(&folder)?;

    println!("Saved: summaries use {name} {model}");
    println!("  settings  {}", folder.join(SETTINGS_FILE).display());
    println!(
        "  API keys  {} (only you can read it)",
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
