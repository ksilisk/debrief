//! Summaries through OpenAI, Anthropic, OpenRouter or an OpenAI-compatible
//! server, with the settings and the API keys kept in the app's folder.

mod anthropic;
pub mod config;
mod http;
mod openai;

use std::sync::atomic::AtomicBool;

use debrief_core::summary::{SummarizeError, Summarizer};
use reqwest::blocking::Client;

use config::{ConfigError, Endpoint, Keys, Provider, Settings};
pub use http::HttpError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    /// Listed by Anthropic, OpenRouter and some servers; not by OpenAI.
    pub context_tokens: Option<usize>,
    pub answer_tokens: Option<usize>,
}

impl ModelInfo {
    /// The answer limit for a summary: the model's own, up to [`config::DEFAULT_ANSWER_TOKENS`].
    pub fn answer_limit(&self) -> usize {
        self.answer_tokens
            .map_or(config::DEFAULT_ANSWER_TOKENS, |limit| {
                limit.min(config::DEFAULT_ANSWER_TOKENS)
            })
    }
}

/// OpenAI lists every model it has; these are not for chat.
const NOT_CHAT: [&str; 10] = [
    "audio",
    "realtime",
    "tts",
    "transcribe",
    "image",
    "search",
    "embedding",
    "moderation",
    "instruct",
    "codex",
];

/// The models that can write a summary. Everything, if the filter leaves nothing.
pub fn chat_models(provider: Provider, models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    if provider != Provider::Openai {
        return models;
    }
    let chat: Vec<ModelInfo> = models
        .iter()
        .filter(|model| {
            let id = model.id.as_str();
            let family = id.starts_with("gpt-")
                || id.starts_with("chatgpt")
                || (id.starts_with('o') && id[1..].starts_with(|c: char| c.is_ascii_digit()));
            family && !NOT_CHAT.iter().any(|word| id.contains(word))
        })
        .cloned()
        .collect();
    if chat.is_empty() { models } else { chat }
}

/// Also checks the key: a wrong key fails here. `url` is the API's base URL.
pub fn list_models(
    provider: Provider,
    url: &str,
    key: Option<&str>,
) -> Result<Vec<ModelInfo>, HttpError> {
    let client = http::client()?;
    match provider {
        Provider::Anthropic => anthropic::models(&client, url, key.unwrap_or_default()),
        Provider::Openrouter => {
            openai::check_key(&client, url, key)?;
            openai::models(&client, url, key)
        }
        Provider::Openai | Provider::Server => openai::models(&client, url, key),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("no {0} API key is saved")]
    NoKey(String),
    #[error("no summary model is chosen")]
    NoModel,
    #[error(transparent)]
    Http(#[from] HttpError),
}

/// The model from the settings, with the saved key and the prompt file.
pub struct RemoteModel {
    client: Client,
    endpoint: Endpoint,
    /// The id the provider knows, such as `claude-opus-5-5`.
    model_id: String,
    /// Provider or server, and the id; saved with the step.
    model: String,
    /// A server may need none.
    key: Option<String>,
    /// The default prompt, from `summary-prompt.txt` in the app's folder.
    prompt: String,
    context_tokens: usize,
    answer_tokens: usize,
}

impl RemoteModel {
    /// `None` when summaries aren't set up.
    pub fn from_settings() -> Result<Option<Self>, SetupError> {
        let folder = config::folder()?;
        let settings = Settings::load(&folder)?.summary;
        let Some(endpoint) = settings.endpoint() else {
            return Ok(None);
        };
        if settings.model.trim().is_empty() {
            return Err(SetupError::NoModel);
        }
        let key = Keys::load(&folder)?
            .get(&endpoint.key_name)
            .map(str::to_string);
        if key.is_none() && endpoint.needs_key() {
            return Err(SetupError::NoKey(endpoint.name));
        }
        Ok(Some(Self {
            client: http::client()?,
            model: format!("{} {}", endpoint.name, settings.model),
            endpoint,
            model_id: settings.model,
            key,
            prompt: config::prompt(&folder)?,
            context_tokens: settings.context_tokens,
            answer_tokens: settings.answer_tokens,
        }))
    }
}

impl Summarizer for RemoteModel {
    fn model(&self) -> &str {
        &self.model
    }

    fn prompt(&self) -> &str {
        &self.prompt
    }

    fn context_tokens(&self) -> usize {
        self.context_tokens
    }

    fn answer_tokens(&self) -> usize {
        self.answer_tokens
    }

    fn summarize(
        &mut self,
        prompt: &str,
        transcript: &str,
        on_text: &mut dyn FnMut(usize),
        cancelled: &AtomicBool,
    ) -> Result<String, SummarizeError> {
        let key = self.key.as_deref();
        let result = match self.endpoint.provider {
            Provider::Anthropic => anthropic::summarize(
                &self.client,
                &self.endpoint.url,
                key.unwrap_or_default(),
                &self.model_id,
                self.answer_tokens,
                prompt,
                transcript,
                on_text,
                cancelled,
            ),
            Provider::Openai | Provider::Openrouter | Provider::Server => openai::summarize(
                &self.client,
                &self.endpoint.url,
                key,
                &self.model_id,
                prompt,
                transcript,
                on_text,
                cancelled,
            ),
        };
        match result {
            Ok(Some(text)) => Ok(text),
            Ok(None) => Err(SummarizeError::Cancelled),
            Err(error) => Err(SummarizeError::Failed(Box::new(error))),
        }
    }
}
