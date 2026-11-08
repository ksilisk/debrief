//! OpenAI's chat completions API, streamed.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::ModelInfo;
use crate::http::{self, Event, Events, HttpError};

const BASE: &str = "https://api.openai.com/v1";

pub fn models(client: &Client, key: &str) -> Result<Vec<ModelInfo>, HttpError> {
    let response = http::send(|| client.get(format!("{BASE}/models")).bearer_auth(key))?;
    let body: Value = serde_json::from_str(&response.text().map_err(HttpError::Network)?)
        .map_err(|error| HttpError::Answer(format!("unexpected model list: {error}")))?;
    let mut models: Vec<ModelInfo> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| model["id"].as_str())
        .map(|id| ModelInfo {
            id: id.to_string(),
            context_tokens: None,
            answer_tokens: None,
        })
        .collect();
    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

/// `Ok(None)` when cancelled.
pub fn summarize(
    client: &Client,
    key: &str,
    model: &str,
    prompt: &str,
    transcript: &str,
    on_text: &mut dyn FnMut(usize),
    cancelled: &AtomicBool,
) -> Result<Option<String>, HttpError> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": transcript},
        ],
        "stream": true,
    });
    let bytes = serde_json::to_vec(&body).map_err(|error| HttpError::Answer(error.to_string()))?;
    let response = http::send(|| {
        client
            .post(format!("{BASE}/chat/completions"))
            .bearer_auth(key)
            .header("content-type", "application/json")
            .body(bytes.clone())
    })?;
    read_stream(Events::new(response), on_text, cancelled)
}

fn read_stream(
    events: impl Iterator<Item = io::Result<Event>>,
    on_text: &mut dyn FnMut(usize),
    cancelled: &AtomicBool,
) -> Result<Option<String>, HttpError> {
    let mut text = String::new();
    let mut characters = 0;
    for event in events {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let event = event.map_err(HttpError::Stream)?;
        if event.data == "[DONE]" {
            break;
        }
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|error| HttpError::Answer(format!("unexpected answer: {error}")))?;
        if let Some(message) = value.pointer("/error/message").and_then(Value::as_str) {
            return Err(HttpError::Answer(message.to_string()));
        }
        let choice = &value["choices"][0];
        if let Some(delta) = choice.pointer("/delta/content").and_then(Value::as_str) {
            text.push_str(delta);
            characters += delta.chars().count();
            on_text(characters);
        }
        match choice["finish_reason"].as_str() {
            Some("length") => {
                return Err(HttpError::Answer(
                    "the summary was cut off at the model's answer limit".into(),
                ));
            }
            Some("content_filter") => {
                return Err(HttpError::Answer(
                    "the provider declined to summarize this transcript".into(),
                ));
            }
            _ => {}
        }
    }
    if text.trim().is_empty() {
        return Err(HttpError::Answer(
            "the model returned an empty summary".into(),
        ));
    }
    Ok(Some(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(stream: &str) -> Result<Option<String>, HttpError> {
        let mut seen = Vec::new();
        let result = read_stream(
            Events::new(stream.as_bytes()),
            &mut |n| seen.push(n),
            &AtomicBool::new(false),
        );
        if let Ok(Some(text)) = &result {
            assert_eq!(seen.last(), Some(&text.chars().count()));
        }
        result
    }

    #[test]
    fn joins_the_streamed_text() {
        let stream = concat!(
            "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"## Итог\\n\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Релиз в понедельник.\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        assert_eq!(
            read(stream).expect("read"),
            Some("## Итог\nРелиз в понедельник.".into())
        );
    }

    #[test]
    fn a_cut_off_answer_is_an_error() {
        let stream = "data: {\"choices\":[{\"delta\":{\"content\":\"Half\"},\"finish_reason\":\"length\"}]}\n\n";
        assert!(matches!(read(stream), Err(HttpError::Answer(_))));
    }
}
