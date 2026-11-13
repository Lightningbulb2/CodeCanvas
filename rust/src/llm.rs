//! Streaming chat against Ollama, llama.cpp / OpenAI-compatible servers, and Anthropic.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub type Cancel = Arc<AtomicBool>;

#[derive(Clone, Debug, PartialEq)]
pub enum Wire { Ollama, OpenAi, Anthropic }

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub wire: Wire,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub api_key: Option<String>,
}

impl LlmConfig {
    pub fn new(provider: &str, endpoint: &str, model: &str, temperature: f32, max_tokens: u32, api_key: Option<String>) -> Self {
        let (wire, default) = match provider {
            "ollama" => (Wire::Ollama, "http://localhost:11434"),
            "openai" => (Wire::OpenAi, "http://localhost:1234/v1"),
            "anthropic" => (Wire::Anthropic, "https://api.anthropic.com"),
            _ => (Wire::OpenAi, "http://localhost:8080/v1"),
        };
        let typed = endpoint.trim();
        let mut endpoint = if typed.is_empty() { default } else { typed }.trim_end_matches('/').to_string();
        if provider == "llamacpp" && !endpoint.ends_with("/v1") { endpoint.push_str("/v1"); }
        Self { wire, provider: provider.into(), endpoint, model: model.into(), temperature, max_tokens, api_key }
    }
}

#[derive(Clone, Debug)]
pub struct Message { pub role: &'static str, pub content: String }

fn post(url: &str, cfg: &LlmConfig, body: Value) -> Result<ureq::Response, String> {
    let mut req = ureq::post(url).set("content-type", "application/json");
    match cfg.wire {
        Wire::OpenAi => if let Some(k) = &cfg.api_key { req = req.set("authorization", &format!("Bearer {k}")); },
        Wire::Anthropic => {
            let k = cfg.api_key.as_deref().ok_or("No API key set (see Settings).")?;
            req = req.set("x-api-key", k).set("anthropic-version", "2023-06-01");
        }
        Wire::Ollama => {}
    }
    req.send_string(&body.to_string()).map_err(|e| describe(e, url, cfg))
}

fn describe(e: ureq::Error, url: &str, cfg: &LlmConfig) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let hint = if code == 404 { format!(" {url} doesn't exist. Check the provider and endpoint in Settings.") } else { String::new() };
            format!("HTTP {code}: {}{hint}", body.chars().take(400).collect::<String>())
        }
        ureq::Error::Transport(t) => format!("Couldn't reach the {} server at {} ({t}). Is it running?", cfg.provider, cfg.endpoint),
    }
}

/// Streams a completion, calling `on_delta` per chunk. Returns the full text.
pub fn stream_chat(cfg: &LlmConfig, msgs: &[Message], cancel: &Cancel, on_delta: &mut dyn FnMut(&str), on_thinking: &mut dyn FnMut()) -> Result<String, String> {
    let as_json = |m: &Message| json!({ "role": m.role, "content": m.content });
    let (url, body) = match cfg.wire {
        Wire::Ollama => (format!("{}/api/chat", cfg.endpoint), json!({
            "model": cfg.model, "stream": true, "messages": msgs.iter().map(as_json).collect::<Vec<_>>(),
            "options": { "temperature": cfg.temperature, "num_predict": cfg.max_tokens, "num_ctx": 16384 },
        })),
        Wire::OpenAi => (format!("{}/chat/completions", cfg.endpoint), json!({
            "model": cfg.model, "stream": true, "temperature": cfg.temperature, "max_tokens": cfg.max_tokens,
            "messages": msgs.iter().map(as_json).collect::<Vec<_>>(),
        })),
        Wire::Anthropic => (format!("{}/v1/messages", cfg.endpoint), json!({
            "model": cfg.model, "stream": true, "max_tokens": cfg.max_tokens,
            "system": msgs.iter().filter(|m| m.role == "system").map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n\n"),
            "messages": msgs.iter().filter(|m| m.role != "system").map(as_json).collect::<Vec<_>>(),
        })),
    };
    let reader = BufReader::new(post(&url, cfg, body)?.into_reader());
    let mut full = String::new();
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) { return Err("Stopped.".into()); }
        let line = line.map_err(|e| e.to_string())?;
        match parse_line(&cfg.wire, &line)? {
            Chunk::Text(t) => { full.push_str(&t); on_delta(&t); }
            Chunk::Thinking => on_thinking(),
            Chunk::Done => break,
            Chunk::None => {}
        }
    }
    Ok(full)
}

#[derive(Debug, PartialEq)]
pub enum Chunk { Text(String), Thinking, Done, None }

pub fn parse_line(wire: &Wire, line: &str) -> Result<Chunk, String> {
    if *wire == Wire::Ollama {
        if line.trim().is_empty() { return Ok(Chunk::None); }
        let j: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        if let Some(e) = j["error"].as_str() { return Err(e.to_string()); }
        if let Some(t) = j["message"]["content"].as_str().filter(|t| !t.is_empty()) { return Ok(Chunk::Text(t.into())); }
        return Ok(if j["done"].as_bool() == Some(true) { Chunk::Done } else { Chunk::None });
    }
    let Some(data) = line.strip_prefix("data:").map(str::trim) else { return Ok(Chunk::None) };
    if data == "[DONE]" { return Ok(Chunk::Done); }
    let Ok(j) = serde_json::from_str::<Value>(data) else { return Ok(Chunk::None) };
    if *wire == Wire::OpenAi {
        let d = &j["choices"][0]["delta"];
        if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) { return Ok(Chunk::Text(t.into())); }
        return Ok(if d["reasoning_content"].is_string() { Chunk::Thinking } else { Chunk::None });
    }
    match j["type"].as_str() {
        Some("content_block_delta") if j["delta"]["type"] == "text_delta" => Ok(Chunk::Text(j["delta"]["text"].as_str().unwrap_or("").into())),
        Some("error") => Err(j["error"]["message"].as_str().unwrap_or("Anthropic error").into()),
        Some("message_stop") => Ok(Chunk::Done),
        _ => Ok(Chunk::None),
    }
}

pub fn list_models(cfg: &LlmConfig) -> Vec<String> {
    let url = match cfg.wire {
        Wire::Ollama => format!("{}/api/tags", cfg.endpoint),
        Wire::OpenAi => format!("{}/models", cfg.endpoint),
        Wire::Anthropic => format!("{}/v1/models", cfg.endpoint),
    };
    let mut req = ureq::get(&url).timeout(std::time::Duration::from_secs(5));
    if let Some(k) = &cfg.api_key {
        req = if cfg.wire == Wire::Anthropic { req.set("x-api-key", k).set("anthropic-version", "2023-06-01") } else { req.set("authorization", &format!("Bearer {k}")) };
    }
    let Some(j) = req.call().ok().and_then(|r| r.into_json::<Value>().ok()) else { return Vec::new() };
    let (list, key) = if cfg.wire == Wire::Ollama { (&j["models"], "name") } else { (&j["data"], "id") };
    list.as_array().map(|a| a.iter().filter_map(|m| m[key].as_str().map(String::from)).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llamacpp_endpoint_is_normalised() {
        assert_eq!(LlmConfig::new("llamacpp", "", "m", 0., 1, None).endpoint, "http://localhost:8080/v1");
        assert_eq!(LlmConfig::new("llamacpp", "http://h:9/", "m", 0., 1, None).endpoint, "http://h:9/v1");
        assert_eq!(LlmConfig::new("llamacpp", "", "m", 0., 1, None).wire, Wire::OpenAi);
    }

    #[test]
    fn parses_each_wire() {
        assert_eq!(parse_line(&Wire::OpenAi, r#"data: {"choices":[{"delta":{"content":"hi"}}]}"#), Ok(Chunk::Text("hi".into())));
        assert_eq!(parse_line(&Wire::OpenAi, r#"data: {"choices":[{"delta":{"reasoning_content":"x"}}]}"#), Ok(Chunk::Thinking));
        assert_eq!(parse_line(&Wire::OpenAi, "data: [DONE]"), Ok(Chunk::Done));
        assert_eq!(parse_line(&Wire::Ollama, r#"{"message":{"content":"a"}}"#), Ok(Chunk::Text("a".into())));
        assert_eq!(parse_line(&Wire::Ollama, r#"{"done":true}"#), Ok(Chunk::Done));
        assert_eq!(parse_line(&Wire::Anthropic, r#"data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"b"}}"#), Ok(Chunk::Text("b".into())));
    }
}
