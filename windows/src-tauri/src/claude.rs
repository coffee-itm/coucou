// Claude API client — the same integration as ClaudeService.swift: multi-turn
// chat with web search, and files sent as document/image/text blocks.
//
// Everything happens here rather than in the island: the API key never leaves
// the Credential Manager, and file bytes never cross the IPC boundary.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::secrets;

const ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Server-side fallback: on a policy decline the API retries the same request on
/// a fallback model inside the same call, so the island never shows a dead end.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const MAX_TOKENS: u32 = 4096;
/// Text and code files are inlined; anything larger is skipped, as on macOS.
const MAX_INLINE_TEXT: u64 = 200_000;

pub const DEFAULT_MODEL: &str = "claude-opus-5";

const SYSTEM_PROMPT: &str = "You are Mochi, a personal AI assistant living at the top of the user's screen. \
You have web search access and can help with absolutely anything — research, coding, finding places, recommendations, tasks, questions. \
Respond in the user's language. Be thorough and complete — use as much detail as the task requires. \
No markdown formatting (no **, no ##, no bullet dashes). Use plain text with line breaks.";

#[derive(Default)]
pub struct Chat {
    /// Full multi-turn history, including tool_use / tool_result blocks.
    messages: Mutex<Vec<Value>>,
}

impl Chat {
    pub fn reset(&self) {
        self.messages.lock().unwrap().clear();
    }

    fn is_empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }

    fn push(&self, message: Value) {
        self.messages.lock().unwrap().push(message);
    }

    fn pop(&self) {
        self.messages.lock().unwrap().pop();
    }

    fn snapshot(&self) -> Vec<Value> {
        self.messages.lock().unwrap().clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatContext {
    File { name: String, path: String },
    Window { app_name: String, title: String, url: Option<String> },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsReply {
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

/// One chat turn. Returns the assistant's text, or a message the island shows
/// in the note view.
pub async fn send(
    chat: &Chat,
    model: &str,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let provider = crate::settings::load().chat_provider;
    match provider.as_str() {
        "anthropic" => send_anthropic(chat, model, query, context).await,
        "google" => send_openai_compatible(chat, model, query, context, "google").await,
        "openai" => send_openai_compatible(chat, model, query, context, "openai").await,
        "custom" => send_openai_compatible(chat, model, query, context, "custom").await,
        _ => send_anthropic(chat, model, query, context).await,
    }
}

/// Anthropic API (with web search tool).
async fn send_anthropic(
    chat: &Chat,
    model: &str,
    query: String,
    context: Option<ChatContext>,
) -> Result<ChatReply, String> {
    let key = secrets::get("anthropic-api-key")
        .ok_or_else(|| "API key missing. Open settings.".to_string())?;

    let mut content: Vec<Value> = Vec::new();

    if chat.is_empty() {
        match &context {
            Some(ChatContext::File { name, path }) => {
                if let Some(block) = file_block(path) {
                    content.push(block);
                }
                content.push(json!({ "type": "text", "text": format!("File: {name}") }));
            }
            Some(ChatContext::Window { app_name, title, url }) => {
                let mut text = format!("Context — App: {app_name}, Window: {title}");
                if let Some(url) = url {
                    text.push_str(&format!(", URL: {url}"));
                }
                content.push(json!({ "type": "text", "text": text }));
            }
            None => {}
        }
    }
    content.push(json!({ "type": "text", "text": query }));

    chat.push(json!({ "role": "user", "content": content }));

    let body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": SYSTEM_PROMPT,
        "tools": [{ "type": "web_search_20260209", "name": "web_search", "max_uses": 5 }],
        "fallbacks": "default",
        "messages": chat.snapshot(),
    });

    let response = match call(&key, &body).await {
        Ok(v) => v,
        Err(err) => {
            chat.pop();
            return Err(err);
        }
    };

    if response.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        chat.pop();
        let why = response
            .get("stop_details")
            .and_then(|d| d.get("explanation"))
            .and_then(Value::as_str)
            .unwrap_or("Claude declined this one.");
        return Err(why.to_string());
    }

    let Some(blocks) = response.get("content").and_then(Value::as_array).cloned() else {
        chat.pop();
        return Err("Unexpected API response.".into());
    };

    chat.push(json!({ "role": "assistant", "content": blocks.clone() }));

    let text = blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|b| b.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("No response text.".into());
    }
    Ok(ChatReply { text })
}

/// OpenAI-compatible API (Google Gemini, OpenAI, Custom/FreeLLMAPI).
async fn send_openai_compatible(
    chat: &Chat,
    model: &str,
    query: String,
    context: Option<ChatContext>,
    provider: &str,
) -> Result<ChatReply, String> {
    let settings = crate::settings::load();
    
    let (base_url, key) = match provider {
        "google" => {
            let key = secrets::get("google-api-key")
                .ok_or_else(|| "Google API key missing. Open settings.".to_string())?;
            ("https://generativelanguage.googleapis.com/v1beta/openai".to_string(), key)
        }
        "openai" => {
            let key = secrets::get("openai-api-key")
                .ok_or_else(|| "OpenAI API key missing. Open settings.".to_string())?;
            ("https://api.openai.com/v1".to_string(), key)
        }
        "custom" => {
            let base_url = settings.custom_base_url;
            if base_url.is_empty() {
                return Err("Custom base URL not configured. Open settings.".to_string());
            }
            let key = secrets::get("custom-api-key")
                .ok_or_else(|| "Custom API key missing. Open settings.".to_string())?;
            (normalize_url(&base_url), key)
        }
        _ => return Err(format!("Unknown provider: {provider}")),
    };

    let mut msgs: Vec<Value> = vec![json!({ "role": "system", "content": SYSTEM_PROMPT })];
    
    for m in chat.snapshot() {
        let mut simplified = m.clone();
        if let Some(content) = m.get("content").and_then(|c| c.as_array()) {
            if let Some(text_block) = content.iter().find(|b| b.get("type").and_then(|t| t.as_str()) == Some("text")) {
                if let Some(text) = text_block.get("text").and_then(|t| t.as_str()) {
                    simplified["content"] = json!(text);
                }
            }
        }
        msgs.push(simplified);
    }

    let mut user_text = query;
    if chat.is_empty() {
        if let Some(ctx) = context {
            match ctx {
                ChatContext::File { name, path } => {
                    user_text = format!("File: {name}\n\n{query}");
                }
                ChatContext::Window { app_name, title, url } => {
                    let mut prefix = format!("Context — App: {app_name}, Window: {title}");
                    if let Some(u) = url { prefix.push_str(&format!(", URL: {u}")); }
                    user_text = format!("{prefix}\n\n{query}");
                }
            }
        }
    }
    msgs.push(json!({ "role": "user", "content": user_text }));
    chat.push(json!({ "role": "user", "content": user_text }));

    let body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "messages": msgs,
        "stream": true,
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("{base_url}/chat/completions");
    let mut req = client.post(&url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .json(&body);

    let response = req.send().await.map_err(|e| format!("Network error: {e}"))?;
    
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()))
            .unwrap_or(&text[..text.len().min(200)]);
        return Err(format!("API {status}: {detail}"));
    }

    let mut accumulated = String::new();
    let mut stream = response.bytes_stream();
    
    use futures_util::StreamExt;
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&chunk);
        for line in text.lines() {
            if let Some(delta) = parse_sse_delta(line) {
                accumulated.push_str(&delta);
            }
        }
    }

    // Filter thinking blocks
    let clean = filter_thinking_blocks(&accumulated);
    chat.push(json!({ "role": "assistant", "content": clean.clone() }));
    
    Ok(ChatReply { text: clean })
}

fn normalize_url(url: &str) -> String {
    let mut s = url.trim().to_string();
    while s.ends_with('/') { s.pop(); }
    for suffix in ["/api", "/v1", "/v1/"] {
        if s.ends_with(suffix) { s.truncate(s.len() - suffix.len()); }
    }
    s
}

fn parse_sse_delta(line: &str) -> Option<String> {
    if !line.starts_with("data: ") { return None; }
    let payload = &line[6..];
    if payload == "[DONE]" { return None; }
    let data = payload.as_bytes();
    let json: Value = serde_json::from_slice(data).ok()?;
    let choices = json.get("choices")?.as_array()?;
    let delta = choices.first()?.get("delta")?.as_object()?;
    delta.get("content")?.as_str().map(|s| s.to_string())
}

fn filter_thinking_blocks(text: &str) -> String {
    let pattern = "？？？[\\s\\S]*?回答";
    let regex = regex::Regex::new(pattern).ok()?;
    let range = std::ops::Range { start: 0, end: text.len() };
    let result = regex.replace_all(text, "").to_string();
    // Also handle <think blocks
    let think_pattern = "(?s)<think>.*?</think>";
    let think_regex = regex::Regex::new(think_pattern).ok()?;
    think_regex.replace_all(&result, "").trim().to_string()
}

/// Fetch models from Anthropic API.
pub async fn fetch_anthropic_models(api_key: &str) -> Result<Vec<ModelInfo>, String> {
    let url = "https://api.anthropic.com/v1/models?limit=100";
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    
    if !response.status().is_success() {
        return Err(format!("API error: {}", response.status()));
    }
    
    let data = response.json::<Value>().await.map_err(|e| e.to_string())?;
    let items = data.get("data").and_then(|d| d.as_array()).ok_or("No models data")?;
    
    let models = items.iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            let name = item.get("display_name")?.as_str()?;
            Some(ModelInfo { id: id.to_string(), label: name.to_string() })
        })
        .collect();
    
    Ok(models)
}

/// Fetch models from Google API (OpenAI-compatible endpoint).
pub async fn fetch_google_models(api_key: &str) -> Result<Vec<ModelInfo>, String> {
    fetch_openai_compatible_models("https://generativelanguage.googleapis.com/v1beta/openai", api_key).await
}

/// Fetch models from OpenAI API.
pub async fn fetch_openai_models(api_key: &str) -> Result<Vec<ModelInfo>, String> {
    fetch_openai_compatible_models("https://api.openai.com/v1", api_key).await
}

/// Fetch models from custom OpenAI-compatible endpoint.
pub async fn fetch_custom_models(base_url: &str, api_key: &str) -> Result<Vec<ModelInfo>, String> {
    let url = normalize_url(base_url);
    fetch_openai_compatible_models(&url, api_key).await
}

/// Generic fetch for OpenAI-compatible /models endpoint.
async fn fetch_openai_compatible_models(base_url: &str, api_key: &str) -> Result<Vec<ModelInfo>, String> {
    let url = format!("{base_url}/models");
    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    
    if !response.status().is_success() {
        return Err(format!("API error: {}", response.status()));
    }
    
    let data = response.json::<Value>().await.map_err(|e| e.to_string())?;
    let items = data.get("data").and_then(|d| d.as_array()).ok_or("No models data")?;
    
    let excluded = ["embed", "bge-", "all-minilm", "clip", "rerank", "tts", "whisper", "dall-e", "audio", "image", "moderation", "embedding"];
    
    let models = items.iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            let lower = id.to_lowercase();
            if excluded.iter().any(|e| lower.contains(e)) { return None; }
            Some(ModelInfo { id: id.to_string(), label: id.to_string() })
        })
        .collect();
    
    Ok(models)
}

async fn call(key: &str, body: &Value) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(ENDPOINT)
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("anthropic-beta", FALLBACK_BETA)
        .header("content-type", "application/json")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = response.status();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("Claude API {status}: {detail}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Bad API response: {e}"))
}

/// PDF → document block, image → image block, text/code → inline text.
/// Mirrors readFileAsBlock() in ClaudeService.swift.
fn file_block(path: &str) -> Option<Value> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let media_type = match ext.as_str() {
        "pdf" => Some(("document", "application/pdf")),
        "jpg" | "jpeg" => Some(("image", "image/jpeg")),
        "png" => Some(("image", "image/png")),
        "gif" => Some(("image", "image/gif")),
        "webp" => Some(("image", "image/webp")),
        _ => None,
    };

    if let Some((block_type, media)) = media_type {
        let bytes = std::fs::read(path).ok()?;
        return Some(json!({
            "type": block_type,
            "source": { "type": "base64", "media_type": media, "data": base64(&bytes) },
        }));
    }

    let len = std::fs::metadata(path).ok()?.len();
    if len > MAX_INLINE_TEXT {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(json!({ "type": "text", "text": format!("File contents:\n{text}") }))
}

/// Small standalone base64 encoder — not worth another dependency.
/// Also used for Stripe's basic auth.
pub(crate) fn base64_for(bytes: &[u8]) -> String {
    base64(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
