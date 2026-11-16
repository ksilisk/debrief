//! Files in the app's folder: `settings.json`, `summary-prompt.txt` and
//! `keys.json` (the API keys, readable only by the owner).

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use debrief_core::APP_ID;
use debrief_core::cleanup::{Delete, Rule};
use debrief_core::meeting::{self, Language};
use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "settings.json";
pub const PROMPT_FILE: &str = "summary-prompt.txt";
pub const KEYS_FILE: &str = "keys.json";

pub const DEFAULT_CONTEXT_TOKENS: usize = 128_000;
pub const DEFAULT_ANSWER_TOKENS: usize = 32_000;

pub const DEFAULT_PROMPT: &str = "\
You will get the transcript of a meeting. \"Me\" is the person who recorded it; \"Them\" is \
everyone else on the call. Lines starting with ★ are moments I marked as important.

Write a summary in the main language of the meeting, with these parts:

- Summary: two to four sentences on what the meeting was about and how it ended.
- Decisions: what was agreed.
- Action items: who does what, and by when if a date was said.
- Open questions: what is still unresolved.

Use Markdown headings for the parts and leave out a part that has nothing in it. Start with \
the first part; don't add a title. Include only what was actually said, and keep names, \
numbers and dates as they appear.
";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("HOME is not set")]
    NoHome,
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{} is not valid: {source}", path.display())]
    Invalid {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("could not write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

/// `~/Library/Application Support/<app id>`, shared with the app.
pub fn folder() -> Result<PathBuf, ConfigError> {
    let home = std::env::var_os("HOME").ok_or(ConfigError::NoHome)?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support")
        .join(APP_ID))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Openai,
    Anthropic,
    Openrouter,
    /// One of the OpenAI-compatible servers the user added, such as Ollama or LM Studio.
    Server,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Openai => "openai",
            Provider::Anthropic => "anthropic",
            Provider::Openrouter => "openrouter",
            Provider::Server => "server",
        }
    }

    /// The API's base URL; a server's is in the settings.
    pub fn url(self) -> Option<&'static str> {
        match self {
            Provider::Openai => Some("https://api.openai.com/v1"),
            Provider::Anthropic => Some("https://api.anthropic.com/v1"),
            Provider::Openrouter => Some("https://openrouter.ai/api/v1"),
            Provider::Server => None,
        }
    }

    /// Chosen first when nothing is chosen yet and the provider lists it.
    pub fn suggested_model(self) -> Option<&'static str> {
        match self {
            Provider::Openai => Some("gpt-5-mini"),
            Provider::Anthropic => Some("claude-opus-5-5"),
            Provider::Openrouter | Provider::Server => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown provider `{0}`; use openai, anthropic, openrouter or server")]
pub struct UnknownProvider(String);

impl FromStr for Provider {
    type Err = UnknownProvider;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.trim().to_lowercase().as_str() {
            "openai" => Ok(Provider::Openai),
            "anthropic" | "claude" => Ok(Provider::Anthropic),
            "openrouter" => Ok(Provider::Openrouter),
            "server" => Ok(Provider::Server),
            other => Err(UnknownProvider(other.to_string())),
        }
    }
}

/// An OpenAI-compatible server the user added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub id: u32,
    /// The user's name for it; empty shows the host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// The API's base URL, usually ending in `/v1`.
    pub url: String,
}

impl Server {
    /// The label, or the host when there is none.
    pub fn name(&self) -> String {
        match self.label.trim() {
            "" => host(&self.url).unwrap_or_else(|| self.url.clone()),
            label => label.to_string(),
        }
    }

    /// Its key's name in `keys.json`.
    pub fn key_name(&self) -> String {
        server_key_name(self.id)
    }
}

pub fn server_key_name(id: u32) -> String {
    format!("server-{id}")
}

#[derive(Debug, thiserror::Error)]
#[error("enter the full address, such as http://localhost:11434/v1")]
pub struct BadUrl;

/// A server's base URL as typed, checked and without the trailing slash.
pub fn server_url(text: &str) -> Result<String, BadUrl> {
    let text = text.trim().trim_end_matches('/');
    let url = reqwest::Url::parse(text).map_err(|_| BadUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none_or(str::is_empty) {
        return Err(BadUrl);
    }
    Ok(text.to_string())
}

/// `localhost:11434`, or just the host when the URL has no port.
pub fn host(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url).ok()?;
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

/// Where summaries are sent: a provider's API or one of the servers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub provider: Provider,
    pub url: String,
    /// Its key's name in `keys.json`.
    pub key_name: String,
    /// `openai`, or the server's label or host.
    pub name: String,
}

impl Endpoint {
    /// A server's key is optional.
    pub fn needs_key(&self) -> bool {
        self.provider != Provider::Server
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub summary: SummarySettings,
    #[serde(default)]
    pub recording: RecordingSettings,
    #[serde(default)]
    pub transcription: TranscriptionSettings,
    #[serde(default)]
    pub cleanup: CleanupSettings,
    /// Where new meetings go; `None` uses `~/Documents/Debrief`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meetings_folder: Option<PathBuf>,
}

/// What the storage cleanup deletes, and after how many days.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CleanupSettings {
    /// `None` never deletes anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_days: Option<u32>,
    pub delete: Delete,
}

impl CleanupSettings {
    pub fn rule(self) -> Option<Rule> {
        self.after_days.map(|after_days| Rule {
            after_days,
            delete: self.delete,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionSettings {
    /// The Whisper model's name; `None` uses the default one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// The app's last choices on the New recording screen, used for the next one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    /// Off for a talk heard only through the Mac.
    pub record_microphone: bool,
    /// A microphone ID; `None` uses the system default.
    pub microphone: Option<String>,
    /// Off for a meeting in the room or a recording on your own.
    pub record_system_audio: bool,
    /// The bundle ID of the one app whose sound is recorded; `None` records all apps.
    pub app: Option<String>,
    pub video: bool,
    pub fps: u32,
    /// A screen ID; `None` records the main screen.
    pub screen: Option<u32>,
    pub language: Language,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            record_microphone: true,
            microphone: None,
            record_system_audio: true,
            app: None,
            video: true,
            fps: 5,
            screen: None,
            language: Language::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummarySettings {
    /// `None` until summaries are set up.
    pub provider: Option<Provider>,
    /// Which of `servers`, with [`Provider::Server`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<u32>,
    #[serde(default)]
    pub model: String,
    /// How many tokens the model reads at once.
    #[serde(default = "default_context")]
    pub context_tokens: usize,
    /// The most the answer may take, thinking included.
    #[serde(default = "default_answer")]
    pub answer_tokens: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<Server>,
}

fn default_context() -> usize {
    DEFAULT_CONTEXT_TOKENS
}

fn default_answer() -> usize {
    DEFAULT_ANSWER_TOKENS
}

impl Default for SummarySettings {
    fn default() -> Self {
        Self {
            provider: None,
            server: None,
            model: String::new(),
            context_tokens: DEFAULT_CONTEXT_TOKENS,
            answer_tokens: DEFAULT_ANSWER_TOKENS,
            servers: Vec::new(),
        }
    }
}

impl SummarySettings {
    /// `None` until summaries are set up.
    pub fn endpoint(&self) -> Option<Endpoint> {
        match self.provider? {
            Provider::Server => {
                let server = self.find_server(self.server?)?;
                Some(Endpoint {
                    provider: Provider::Server,
                    url: server.url.clone(),
                    key_name: server.key_name(),
                    name: server.name(),
                })
            }
            provider => Some(Endpoint {
                provider,
                url: provider.url()?.to_string(),
                key_name: provider.name().to_string(),
                name: provider.name().to_string(),
            }),
        }
    }

    pub fn find_server(&self, id: u32) -> Option<&Server> {
        self.servers.iter().find(|server| server.id == id)
    }

    /// Changes the server with `id`, or adds a new one; returns its id.
    pub fn put_server(&mut self, id: Option<u32>, label: &str, url: &str) -> u32 {
        let label = label.trim().to_string();
        if let Some(server) = self.servers.iter_mut().find(|server| Some(server.id) == id) {
            server.label = label;
            server.url = url.to_string();
            return server.id;
        }
        let id = self
            .servers
            .iter()
            .map(|server| server.id)
            .max()
            .unwrap_or(0)
            + 1;
        self.servers.push(Server {
            id,
            label,
            url: url.to_string(),
        });
        id
    }

    /// Removes the server; summaries stop being set up if they used it.
    pub fn remove_server(&mut self, id: u32) -> Option<Server> {
        let index = self.servers.iter().position(|server| server.id == id)?;
        if self.provider == Some(Provider::Server) && self.server == Some(id) {
            self.provider = None;
            self.server = None;
            self.model.clear();
        }
        Some(self.servers.remove(index))
    }
}

impl Settings {
    /// The chosen meetings folder, or `~/Documents/Debrief`.
    pub fn library(&self) -> Result<PathBuf, ConfigError> {
        match &self.meetings_folder {
            Some(folder) => Ok(folder.clone()),
            None => meeting::default_library().ok_or(ConfigError::NoHome),
        }
    }

    /// A missing file gives the defaults.
    pub fn load(folder: &Path) -> Result<Self, ConfigError> {
        match read_optional(&folder.join(SETTINGS_FILE))? {
            Some(text) => parse(&folder.join(SETTINGS_FILE), &text),
            None => Ok(Self::default()),
        }
    }

    pub fn save(&self, folder: &Path) -> Result<(), ConfigError> {
        let path = folder.join(SETTINGS_FILE);
        let mut json = serde_json::to_vec_pretty(self).map_err(|e| write_error(&path, e.into()))?;
        json.push(b'\n');
        write_file(&path, &json, None)
    }
}

/// Where new meetings go, from the saved settings.
pub fn library() -> Result<PathBuf, ConfigError> {
    Settings::load(&folder()?)?.library()
}

/// The user's summary instructions, created from [`DEFAULT_PROMPT`] the first time.
pub fn prompt(folder: &Path) -> Result<String, ConfigError> {
    let path = folder.join(PROMPT_FILE);
    match read_optional(&path)? {
        Some(text) => Ok(text),
        None => {
            write_file(&path, DEFAULT_PROMPT.as_bytes(), None)?;
            Ok(DEFAULT_PROMPT.to_string())
        }
    }
}

/// API keys by provider name, or by [`Server::key_name`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Keys(BTreeMap<String, String>);

impl Keys {
    pub fn load(folder: &Path) -> Result<Self, ConfigError> {
        let path = folder.join(KEYS_FILE);
        match read_optional(&path)? {
            Some(text) => parse(&path, &text),
            None => Ok(Self::default()),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .filter(|key| !key.is_empty())
    }

    pub fn set(&mut self, name: &str, key: String) {
        self.0.insert(name.to_string(), key);
    }

    pub fn remove(&mut self, name: &str) {
        self.0.remove(name);
    }

    /// Readable and writable only by the owner (mode 600).
    pub fn save(&self, folder: &Path) -> Result<(), ConfigError> {
        let path = folder.join(KEYS_FILE);
        let mut json = serde_json::to_vec_pretty(self).map_err(|e| write_error(&path, e.into()))?;
        json.push(b'\n');
        write_file(&path, &json, Some(0o600))
    }
}

/// `sk-…abcd`: enough to recognize a key without showing it.
pub fn masked(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 12 {
        return "…".to_string();
    }
    let start: String = chars[..3].iter().collect();
    let end: String = chars[chars.len() - 4..].iter().collect();
    format!("{start}…{end}")
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(path: &Path, text: &str) -> Result<T, ConfigError> {
    serde_json::from_str(text).map_err(|source| ConfigError::Invalid {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes a temporary file and renames it. With `mode`, the file is created
/// with those permissions, so the content is never readable by others.
fn write_file(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| write_error(parent, source))?;
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let _ = fs::remove_file(&temp);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options
        .open(&temp)
        .map_err(|source| write_error(&temp, source))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| write_error(&temp, source))?;
    fs::rename(&temp, path).map_err(|source| write_error(path, source))
}

fn write_error(path: &Path, source: io::Error) -> ConfigError {
    ConfigError::Write {
        path: path.to_path_buf(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("debrief-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn settings_default_until_saved() {
        let folder = temp_folder("settings");
        assert_eq!(Settings::load(&folder).expect("load"), Settings::default());
        let mut settings = Settings::default();
        settings.summary.provider = Some(Provider::Server);
        settings.summary.model = "qwen3:14b".into();
        let id = settings
            .summary
            .put_server(None, "Ollama", "http://localhost:11434/v1");
        settings.summary.server = Some(id);
        settings.recording.app = Some("us.zoom.xos".into());
        settings.recording.language = Language::Auto;
        settings.transcription.model = Some("large-v3".into());
        settings.cleanup = CleanupSettings {
            after_days: Some(90),
            delete: Delete::Meeting,
        };
        settings.meetings_folder = Some(PathBuf::from("/Volumes/Work/Meetings"));
        settings.save(&folder).expect("save");
        assert_eq!(Settings::load(&folder).expect("load"), settings);
    }

    #[test]
    fn a_file_without_recording_choices_gets_the_defaults() {
        let folder = temp_folder("settings-old");
        fs::create_dir_all(&folder).expect("create");
        fs::write(
            folder.join(SETTINGS_FILE),
            r#"{"summary": {"provider": "openai", "model": "gpt-5", "on_by_default": true}}"#,
        )
        .expect("write");
        let settings = Settings::load(&folder).expect("load");
        assert_eq!(settings.summary.model, "gpt-5");
        assert_eq!(settings.summary.servers, Vec::new());
        assert_eq!(settings.recording, RecordingSettings::default());
        assert_eq!(settings.transcription, TranscriptionSettings::default());
        assert_eq!(settings.cleanup.rule(), None);
        assert_eq!(settings.library().ok(), meeting::default_library());
    }

    #[test]
    fn the_prompt_file_starts_with_the_default() {
        let folder = temp_folder("prompt");
        assert_eq!(prompt(&folder).expect("prompt"), DEFAULT_PROMPT);
        fs::write(folder.join(PROMPT_FILE), "Just list the decisions.").expect("write");
        assert_eq!(prompt(&folder).expect("prompt"), "Just list the decisions.");
    }

    #[cfg(unix)]
    #[test]
    fn keys_are_readable_only_by_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let folder = temp_folder("keys");
        let mut keys = Keys::default();
        keys.set("openai", "sk-test-0123456789".into());
        keys.save(&folder).expect("save");
        let mode = fs::metadata(folder.join(KEYS_FILE))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let loaded = Keys::load(&folder).expect("load");
        assert_eq!(loaded.get("openai"), Some("sk-test-0123456789"));
        assert_eq!(loaded.get("anthropic"), None);
    }

    #[test]
    fn servers_are_added_changed_and_removed() {
        let mut summary = SummarySettings::default();
        let ollama = summary.put_server(None, " Ollama ", "http://localhost:11434/v1");
        let work = summary.put_server(None, "", "https://llm.example.com:8443/v1");
        assert_eq!((ollama, work), (1, 2));
        assert_eq!(
            summary.find_server(ollama).map(Server::name).as_deref(),
            Some("Ollama")
        );
        assert_eq!(
            summary.find_server(work).map(Server::name).as_deref(),
            Some("llm.example.com:8443")
        );

        summary.provider = Some(Provider::Server);
        summary.server = Some(work);
        summary.model = "qwen3:14b".into();
        let endpoint = summary.endpoint().expect("endpoint");
        assert_eq!(endpoint.url, "https://llm.example.com:8443/v1");
        assert_eq!(endpoint.key_name, "server-2");
        assert!(!endpoint.needs_key());

        assert_eq!(
            summary.put_server(Some(work), "Work", "https://llm.example.com/v1"),
            work
        );
        assert_eq!(summary.endpoint().expect("endpoint").name, "Work");

        summary.remove_server(work).expect("removed");
        assert_eq!(summary.provider, None);
        assert_eq!(summary.endpoint(), None);
        assert_eq!(summary.put_server(None, "", "http://127.0.0.1:1234/v1"), 2);
    }

    #[test]
    fn server_urls_need_a_scheme_and_a_host() {
        assert_eq!(
            server_url(" http://localhost:11434/v1/ ").ok().as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert!(server_url("localhost:11434/v1").is_err());
        assert!(server_url("ftp://example.com/v1").is_err());
        assert!(server_url("").is_err());
        assert_eq!(
            Provider::Openrouter.url(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            SummarySettings {
                provider: Some(Provider::Openrouter),
                ..SummarySettings::default()
            }
            .endpoint()
            .map(|endpoint| endpoint.key_name)
            .as_deref(),
            Some("openrouter")
        );
    }

    #[test]
    fn masked_keys_show_only_the_ends() {
        assert_eq!(masked("sk-ant-api03-abcdefghijkl-wxyz"), "sk-…wxyz");
        assert_eq!(masked("short"), "…");
    }
}
