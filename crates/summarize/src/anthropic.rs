//! Anthropic's Messages API, streamed.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::blocking::{Client, RequestBuilder};
use serde_json::{Value, json};

use crate::ModelInfo;
use crate::http::{self, Event, Events, HttpError};

const BASE: &str = "https://api.anthropic.com/v1";
const VERSION: &str = "2026-10-21";

fn signed(request: RequestBuilder, key: &str) -> RequestBuilder {
    request
        .header("x-api-key", key)
        .header("anthropic-version", VERSION)
}

/// Newest first, with each model's context window and answer limit.
pub fn models(client: &Client, key: &str) -> Result<Vec<ModelInfo>, HttpError> {
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    loop {
        let mut url = format!("{BASE}/models?limit=1000");
        if let Some(id) = &after {
            url.push_str(&format!("&after_id={id}"));
        }
        let response = http::send(|| signed(client.get(&url), key))?;
        let body: Value = serde_json::from_str(&response.text().map_err(HttpError::Network)?)
            .map_err(|error| HttpError::Answer(format!("unexpected model list: {error}")))?;
        for model in body["data"].as_array().into_iter().flatten() {
            if let Some(id) = model["id"].as_str() {
                models.push(ModelInfo {
                    id: id.to_string(),
                    context_tokens: model["max_input_tokens"].as_u64().map(|n| n as usize),
                    answer_tokens: model["max_tokens"].as_u64().map(|n| n as usize),
                });
            }
        }
        match (body["has_more"].as_bool(), body["last_id"].as_str()) {
            (Some(true), Some(last)) => after = Some(last.to_string()),
            _ => break,
        }
    }
    Ok(models)
}

/// `Ok(None)` when cancelled.
#[allow(clippy::too_many_arguments)]
pub fn summarize(
    client: &Client,
    key: &str,
    model: &str,
    answer_tokens: usize,
    prompt: &str,
    transcript: &str,
    on_text: &mut dyn FnMut(usize),
    cancelled: &AtomicBool,
) -> Result<Option<String>, HttpError> {
    let body = json!({
        "model": model,
        "max_tokens": answer_tokens,
        "system": prompt,
        "messages": [{"role": "user", "content": transcript}],
        "stream": true,
    });
    let bytes = serde_json::to_vec(&body).map_err(|error| HttpError::Answer(error.to_string()))?;
    let response = http::send(|| {
        signed(client.post(format!("{BASE}/messages")), key)
            .header("content-type", "application/json")
            .body(bytes.clone())
    })?;
    read_stream(Events::new(response), on_text, cancelled)
}

/// Text arrives in `text_delta` events; thinking blocks are skipped.
fn read_stream(
    events: impl Iterator<Item = io::Result<Event>>,
    on_text: &mut dyn FnMut(usize),
    cancelled: &AtomicBool,
) -> Result<Option<String>, HttpError> {
    let mut text = String::new();
    let mut characters = 0;
    let mut stop_reason = None;
    for event in events {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let event = event.map_err(HttpError::Stream)?;
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|error| HttpError::Answer(format!("unexpected answer: {error}")))?;
        match value["type"].as_str() {
            Some("content_block_delta") => {
                if value.pointer("/delta/type").and_then(Value::as_str) == Some("text_delta")
                    && let Some(delta) = value.pointer("/delta/text").and_then(Value::as_str)
                {
                    text.push_str(delta);
                    characters += delta.chars().count();
                    on_text(characters);
                }
            }
            Some("message_delta") => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    stop_reason = Some(reason.to_string());
                }
            }
            Some("error") => {
                let message = value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("the provider reported an error");
                return Err(HttpError::Answer(message.to_string()));
            }
            Some("message_stop") => break,
            _ => {}
        }
    }
    match stop_reason.as_deref() {
        Some("refusal") => Err(HttpError::Answer(
            "the model declined to summarize this transcript".into(),
        )),
        Some("max_tokens") => Err(HttpError::Answer(
            "the summary was cut off at the answer limit".into(),
        )),
        _ if text.trim().is_empty() => Err(HttpError::Answer(
            "the model returned an empty summary".into(),
        )),
        _ => Ok(Some(text)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(stream: &str) -> Result<Option<String>, HttpError> {
        read_stream(
            Events::new(stream.as_bytes()),
            &mut |_| {},
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn joins_text_and_skips_thinking() {
        let stream = concat!(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"\"}}\n\n",
            "event: ping\ndata: {\"type\":\"ping\"}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"## Summary\\n\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Release on Monday.\"}}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":12}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        );
        assert_eq!(
            read(stream).expect("read"),
            Some("## Summary\nRelease on Monday.".into())
        );
    }

    #[test]
    fn a_refusal_and_a_stream_error_are_errors() {
        let refusal = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\"}}\n\n";
        assert!(matches!(read(refusal), Err(HttpError::Answer(_))));
        let error = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        match read(error) {
            Err(HttpError::Answer(message)) => assert_eq!(message, "Overloaded"),
            other => panic!("expected an error, got {other:?}"),
        }
    }
}
