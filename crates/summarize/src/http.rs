//! What both providers share: the HTTP client, retries, error messages and the
//! server-sent events their streams use.

use std::io::{self, BufRead, BufReader, Read};
use std::thread;
use std::time::Duration;

use reqwest::blocking::{Client, RequestBuilder, Response};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A long meeting can take a few minutes to summarize, thinking included.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const ATTEMPTS: u32 = 3;
const MAX_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("could not reach the provider: {0}")]
    Network(#[source] reqwest::Error),
    #[error("the API key was rejected ({status}): {message}")]
    Key { status: u16, message: String },
    #[error("the provider answered {status}: {message}")]
    Status { status: u16, message: String },
    #[error("the answer broke off: {0}")]
    Stream(#[source] io::Error),
    #[error("{0}")]
    Answer(String),
}

pub fn client() -> Result<Client, HttpError> {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(HttpError::Network)
}

/// Sends the request, retrying a few times when the provider is busy (429,
/// 5xx) or unreachable. `request` builds a fresh request for each attempt.
pub fn send(request: impl Fn() -> RequestBuilder) -> Result<Response, HttpError> {
    let mut attempt = 1;
    loop {
        let retry = match request().send() {
            Ok(response) if response.status().is_success() => return Ok(response),
            Ok(response) => {
                let status = response.status().as_u16();
                let wait = retry_after(&response);
                let message = error_message(&response.text().unwrap_or_default());
                if status == 429 || status >= 500 {
                    (wait, HttpError::Status { status, message })
                } else if status == 401 || status == 403 {
                    return Err(HttpError::Key { status, message });
                } else {
                    return Err(HttpError::Status { status, message });
                }
            }
            Err(error) if error.is_connect() || error.is_timeout() => {
                (None, HttpError::Network(error))
            }
            Err(error) => return Err(HttpError::Network(error)),
        };
        if attempt >= ATTEMPTS {
            return Err(retry.1);
        }
        let backoff = Duration::from_secs(2u64.pow(attempt));
        thread::sleep(retry.0.unwrap_or(backoff).min(MAX_WAIT));
        attempt += 1;
    }
}

fn retry_after(response: &Response) -> Option<Duration> {
    let seconds = response
        .headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    Some(Duration::from_secs_f64(seconds.max(0.0)))
}

/// Both providers answer errors as `{"error": {"message": ...}}`.
pub fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(|message| message.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            body.chars()
                .take(300)
                .collect::<String>()
                .trim()
                .to_string()
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub name: Option<String>,
    pub data: String,
}

/// Server-sent events: `event:` and `data:` lines, ended by a blank line.
pub struct Events<R> {
    lines: io::Lines<BufReader<R>>,
}

impl<R: Read> Events<R> {
    pub fn new(reader: R) -> Self {
        Self {
            lines: BufReader::new(reader).lines(),
        }
    }
}

impl<R: Read> Iterator for Events<R> {
    type Item = io::Result<Event>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut name = None;
        let mut data: Option<String> = None;
        loop {
            let line = match self.lines.next() {
                Some(Ok(line)) => line,
                Some(Err(error)) => return Some(Err(error)),
                None => return data.map(|data| Ok(Event { name, data })),
            };
            if line.is_empty() {
                if let Some(data) = data.take() {
                    return Some(Ok(Event { name, data }));
                }
                name = None;
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                name = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("data:") {
                let value = value.strip_prefix(' ').unwrap_or(value);
                match &mut data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(value);
                    }
                    None => data = Some(value.to_string()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_named_and_unnamed_events() {
        let stream = "event: message_start\ndata: {\"a\":1}\n\n: ping\n\ndata: {\"b\":2}\ndata: more\n\ndata: [DONE]\n\n";
        let events: Vec<Event> = Events::new(stream.as_bytes())
            .collect::<Result<_, _>>()
            .expect("events");
        assert_eq!(
            events,
            vec![
                Event {
                    name: Some("message_start".into()),
                    data: "{\"a\":1}".into()
                },
                Event {
                    name: None,
                    data: "{\"b\":2}\nmore".into()
                },
                Event {
                    name: None,
                    data: "[DONE]".into()
                },
            ]
        );
    }

    #[test]
    fn finds_the_message_in_an_error_body() {
        let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens is too large"}}"#;
        assert_eq!(error_message(body), "max_tokens is too large");
        assert_eq!(error_message("Bad Gateway"), "Bad Gateway");
    }
}
