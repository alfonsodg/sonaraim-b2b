//! Pluggable AI inference backend client and incremental delta helpers
//! for the toll-free AI Agent TCP/JSON stream.

use crate::http_api::B2bState;
use crate::outbound_dispatcher::{configured_timeout_ms, post_json_with_body};
use crate::webhooks::json_str;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Starts the toll-free AI Agent stream listener on `host:grpc_port`.
///
/// Wire protocol (honest, no gRPC framing): newline-delimited JSON frames
/// (`sonaraim.ai.v1`) over plain TCP. The client sends one JSON request line
/// (`agent_did`, `prompt`); the server streams one `streaming` frame per
/// inference delta and closes with a `done` frame. Deltas come from the
/// pluggable HTTP inference backend (`B2B_AI_BACKEND_URL` via
/// [`crate::http_api::B2bState::set_ai_backend`], model `B2B_AI_MODEL`,
/// default `sonaraim-default`); when no backend is configured or reachable,
/// the prompt is chunked locally as the fallback.
pub fn start_ai_stream_listener(
    host: &str,
    grpc_port: u16,
    shared: Arc<Mutex<B2bState>>,
) -> io::Result<(String, JoinHandle<()>)> {
    let listener = TcpListener::bind(format!("{host}:{grpc_port}"))?;
    let bound_addr = listener.local_addr()?.to_string();
    let handle = thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let worker_shared = Arc::clone(&shared);
            thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let agent_did = json_str(&req, "agent_did")
                    .unwrap_or_else(|| "did:sonaraim:ai-agent".to_string());
                let prompt = json_str(&req, "prompt").unwrap_or_else(|| "ready".to_string());
                let (backend_url, model) = worker_shared
                    .lock()
                    .map(|guard| (guard.ai_backend_url.clone(), guard.ai_model.clone()))
                    .unwrap_or((None, "sonaraim-default".to_string()));
                let deltas = fetch_inference_deltas(backend_url.as_deref(), &model, &prompt);
                for (idx, delta) in deltas.iter().enumerate() {
                    let frame = format!(
                        "{{\"stream\":\"sonaraim.ai.v1\",\"agent_did\":\"{agent_did}\",\"toll_free\":true,\"status\":\"streaming\",\"delta_index\":{idx},\"delta\":\"{}\"}}\n",
                        delta.replace('"', "'")
                    );
                    if stream.write_all(frame.as_bytes()).is_err() {
                        return;
                    }
                }
                let done = format!(
                    "{{\"stream\":\"sonaraim.ai.v1\",\"agent_did\":\"{agent_did}\",\"toll_free\":true,\"status\":\"done\",\"deltas\":{}}}\n",
                    deltas.len()
                );
                let _ = stream.write_all(done.as_bytes());
                let _ = stream.flush();
            });
        }
    });
    Ok((bound_addr, handle))
}

/// Fetches incremental completion deltas from the pluggable inference backend.
///
/// When `B2B_AI_BACKEND_URL` is set, POSTs `{model, prompt}` and parses the
/// backend's `deltas` array; on any failure (or when unset) falls back to
/// word-chunking the prompt locally so the stream always carries ≥1 delta.
fn fetch_inference_deltas(backend_url: Option<&str>, model: &str, prompt: &str) -> Vec<String> {
    if let Some(url) = backend_url.map(str::trim).filter(|s| !s.is_empty()) {
        let body = format!(
            r#"{{"model":"{model}","prompt":"{}"}}"#,
            prompt.replace('"', "'")
        );
        if let Ok(status_body) = post_json_with_body(url, &body, configured_timeout_ms())
            && let Some(deltas) = parse_deltas(&status_body)
            && !deltas.is_empty()
        {
            return deltas;
        }
    }
    chunk_fallback(prompt)
}

/// Parses a backend `{"deltas":[...]}` string array from a JSON body.
fn parse_deltas(body: &str) -> Option<Vec<String>> {
    let needle = "\"deltas\"";
    let idx = body.find(needle)?;
    let after = body[idx + needle.len()..].find('[')?;
    let mut rest = body[idx + needle.len() + after + 1..].trim_start();
    let mut deltas = Vec::new();
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let mut current = String::new();
        let bytes = rest.as_bytes();
        let mut i = 0;
        let mut end = None;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '\\' && i + 1 < bytes.len() {
                current.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if c == '"' {
                end = Some(i);
                break;
            }
            current.push(c);
            i += 1;
        }
        let end = end?;
        deltas.push(current);
        rest = rest[end + 1..].trim_start();
        if rest.starts_with(',') {
            rest = rest[1..].trim_start();
        } else if rest.starts_with(']') {
            break;
        }
    }
    Some(deltas)
}

/// Splits `text` into incremental fallback chunks (8 chars), subdividing the
/// longest chunk until at least 3 deltas exist or nothing left to split.
fn chunk_fallback(text: &str) -> Vec<String> {
    let mut chunks = chunk_chars(text, 8);
    while chunks.len() < 3 {
        let Some((idx, _)) = chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.chars().count() > 1)
            .max_by_key(|(_, c)| c.len())
        else {
            break;
        };
        let chunk = chunks.remove(idx);
        let mid = chunk.len() / 2;
        let (a, b) = chunk.split_at(mid.max(1));
        chunks.insert(idx, a.to_string());
        chunks.insert(idx + 1, b.to_string());
    }
    if chunks.is_empty() {
        chunks.push("ready".to_string());
    }
    chunks
}

/// Splits `text` into character chunks of at most `size` characters.
fn chunk_chars(text: &str, size: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    chars
        .chunks(size.max(1))
        .map(|c| c.iter().collect())
        .collect()
}
