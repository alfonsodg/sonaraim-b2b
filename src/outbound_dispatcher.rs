//! Outbound HTTP/1.1 JSON webhook dispatcher, retry backoff client, `sonaraim-node`
//! MLS relay forwarder, and toll-free AI Agent stream TCP listener on `B2B_GRPC_PORT`.

use crate::delivery_queue::DeliveryQueue;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use crate::ai_stream::start_ai_stream_listener;

/// Parses an `http://host[:port][/path]` URL into `(host_port, path)`.
fn parse_http_url(url: &str) -> Result<(String, String), String> {
    let trimmed = url.trim();
    let without_scheme = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| format!("unsupported URL scheme in '{trimmed}'"))?;
    let (authority, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };
    if authority.is_empty() {
        return Err("empty URL authority".to_string());
    }
    let host_port = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };
    Ok((host_port, path.to_string()))
}

/// Sends an outbound HTTP/1.1 `POST` request with a JSON body and bounded TCP timeout.
pub fn post_json_with_timeout(
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
    timeout_ms: u64,
) -> Result<u16, String> {
    let (host_port, path) = parse_http_url(url)?;
    let addr: SocketAddr = host_port
        .to_socket_addrs()
        .map_err(|e| format!("failed to resolve '{host_port}': {e}"))?
        .next()
        .ok_or_else(|| format!("no socket address found for '{host_port}'"))?;

    let timeout = Duration::from_millis(timeout_ms.max(25));
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect error: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let mut extra_headers = String::new();
    for (k, v) in headers {
        extra_headers.push_str(&format!("{k}: {v}\r\n"));
    }

    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write error: {e}"))?;
    let _ = stream.flush();

    let mut buf = [0u8; 1024];
    let n = stream
        .read(&mut buf)
        .map_err(|e| format!("read error: {e}"))?;
    if n == 0 {
        return Err("empty HTTP response".to_string());
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    head.lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "invalid HTTP status line".to_string())
}

/// Sends an outbound HTTP/1.1 `POST` request with automatic retry backoff on socket errors or 5xx responses.
pub fn post_json_with_retry(
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
    timeout_ms: u64,
    max_retries: u32,
) -> Result<u16, String> {
    let mut last_err = String::from("unattempted");
    for attempt in 0..=max_retries {
        match post_json_with_timeout(url, headers, body, timeout_ms) {
            Ok(status) if status < 500 => return Ok(status),
            Ok(status) => last_err = format!("upstream 5xx status: {status}"),
            Err(err) => last_err = err,
        }
        if attempt < max_retries {
            thread::sleep(Duration::from_millis(15 * u64::from(attempt + 1)));
        }
    }
    Err(last_err)
}

/// Dispatches a JSON payload to an optional upstream URL or fallback environment variable with retries.
pub fn sync_optional_upstream(explicit_url: Option<String>, env_var: &str, payload: &str) -> bool {
    let target = explicit_url
        .or_else(|| std::env::var(env_var).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(url) = target else {
        return false;
    };
    matches!(
        post_json_with_retry(&url, &[], payload, configured_timeout_ms(), 2),
        Ok(st) if (200..300).contains(&st)
    )
}

/// Returns the configured outbound webhook/relay timeout in milliseconds (`B2B_WEBHOOK_TIMEOUT_MS`).
pub fn configured_timeout_ms() -> u64 {
    std::env::var("B2B_WEBHOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(350)
}

/// Forwards a sealed MLS envelope to `{node_rpc_url}/v1/relay/envelopes`.
/// `node_api_token` is the optional `B2B_NODE_API_TOKEN`: when non-empty an
/// `Authorization: Bearer <token>` header is sent (required by
/// token-protected nodes, fail-closed Bearer since node#14); when empty no
/// header is sent (dev-only, unauthenticated nodes only).
pub fn forward_to_node_relay(
    node_rpc_url: &str,
    node_api_token: &str,
    tenant_did: &str,
    recipient_did: &str,
    seq: u64,
    raw_message_json: &str,
) -> bool {
    forward_to_node_relay_with_retry(
        node_rpc_url,
        node_api_token,
        tenant_did,
        recipient_did,
        seq,
        raw_message_json,
        2,
    )
    .delivered
}

/// Outcome of a relay forwarding attempt with retry accounting.
#[derive(Debug, Clone)]
pub struct RelayOutcome {
    /// Whether the envelope reached the relay with a 2xx status.
    pub delivered: bool,
    /// Total HTTP attempts performed (including the first).
    pub attempts: u32,
    /// Last error message when `delivered` is false (empty when delivered).
    pub last_error: String,
}

/// Builds the sealed MLS relay envelope payload for `{node_rpc_url}/v1/relay/envelopes`.
fn relay_envelope_payload(
    tenant_did: &str,
    recipient_did: &str,
    seq: u64,
    raw_message_json: &str,
) -> (String, String) {
    let url_suffix = "/v1/relay/envelopes".to_string();
    let _ = url_suffix;
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let env_id = format!("env_b2b_{seq}_{now_nanos}");
    let raw = if raw_message_json.trim().is_empty() {
        "{}"
    } else {
        raw_message_json.trim()
    };
    let payload_hex = crate::webhooks::encode_hex(raw.as_bytes());
    let payload = format!(
        r#"{{"envelope_id":"{env_id}","recipient_mailbox_id":"{recipient_did}","recipient_did":"{recipient_did}","tenant_did":"{tenant_did}","payload_hex":"{payload_hex}"}}"#
    );
    (env_id, payload)
}

/// Forwards a sealed MLS envelope with exponential-backoff retries, reporting
/// the real outcome (attempts + last error) instead of a silent boolean.
pub fn forward_to_node_relay_with_retry(
    node_rpc_url: &str,
    node_api_token: &str,
    tenant_did: &str,
    recipient_did: &str,
    seq: u64,
    raw_message_json: &str,
    max_retries: u32,
) -> RelayOutcome {
    let url = format!("{}/v1/relay/envelopes", node_rpc_url.trim_end_matches('/'));
    let (_env_id, payload) =
        relay_envelope_payload(tenant_did, recipient_did, seq, raw_message_json);
    let token = node_api_token.trim();
    let auth_value = format!("Bearer {token}");
    let auth_headers: Vec<(&str, &str)> = if token.is_empty() {
        vec![]
    } else {
        vec![("Authorization", auth_value.as_str())]
    };
    let mut last_error = String::from("unattempted");
    let mut attempts = 0u32;
    for attempt in 0..=max_retries {
        attempts = attempts.saturating_add(1);
        match post_json_with_timeout(&url, &auth_headers, &payload, configured_timeout_ms()) {
            Ok(status) if (200..300).contains(&status) => {
                return RelayOutcome {
                    delivered: true,
                    attempts,
                    last_error: String::new(),
                };
            }
            Ok(status) => last_error = format!("relay upstream {status}"),
            Err(err) => last_error = err,
        }
        if attempt < max_retries {
            thread::sleep(Duration::from_millis(DeliveryQueue::backoff_ms(
                attempt, 100,
            )));
        }
    }
    eprintln!(
        "{{\"level\":\"WARN\",\"service\":\"sonaraim-b2b\",\"component\":\"relay_forwarder\",\"tenant_did\":\"{tenant_did}\",\"error\":\"{last_error}\"}}"
    );
    RelayOutcome {
        delivered: false,
        attempts,
        last_error,
    }
}

/// POSTs a JSON body and returns the response body on 2xx, or an error string.
pub(crate) fn post_json_with_body(
    url: &str,
    body: &str,
    timeout_ms: u64,
) -> Result<String, String> {
    let (host_port, path) = parse_http_url(url)?;
    let addr: SocketAddr = host_port
        .to_socket_addrs()
        .map_err(|e| format!("resolve error: {e}"))?
        .next()
        .ok_or_else(|| "no socket address".to_string())?;
    let timeout = Duration::from_millis(timeout_ms.max(25));
    let mut stream =
        TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("connect error: {e}"))?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {host_port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write error: {e}"))?;
    let _ = stream.flush();
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| format!("read error: {e}"))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, resp_body) = text.split_once("\r\n\r\n").unwrap_or(("", text.as_str()));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if (200..300).contains(&status) {
        Ok(resp_body.to_string())
    } else {
        Err(format!("backend status {status}"))
    }
}
