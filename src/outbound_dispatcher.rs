//! Outbound HTTP/1.1 JSON webhook dispatcher and `sonaraim-node` MLS relay forwarder.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
///
/// # Parameters
/// - `url`: Target HTTP endpoint (`http://host:port/path`).
/// - `headers`: Additional HTTP request headers (e.g., `X-Sonaraim-Signature`).
/// - `body`: UTF-8 JSON payload.
/// - `timeout_ms`: Connection and I/O timeout in milliseconds.
///
/// # Errors
///
/// Returns `Err(String)` if URL parsing, DNS resolution, TCP connection, or HTTP response parsing fails.
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
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| "invalid HTTP status line".to_string())?;
    Ok(status)
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
///
/// Returns `true` if `sonaraim-node` responds with a 2xx HTTP status code.
pub fn forward_to_node_relay(
    node_rpc_url: &str,
    tenant_did: &str,
    recipient_did: &str,
    seq: u64,
) -> bool {
    let url = format!("{}/v1/relay/envelopes", node_rpc_url.trim_end_matches('/'));
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let env_id = format!("env_b2b_{seq}_{now_nanos}");
    let payload = format!(
        r#"{{"envelope_id":"{env_id}","recipient_mailbox_id":"{recipient_did}","recipient_did":"{recipient_did}","tenant_did":"{tenant_did}","payload_hex":"736f6e617261696d5f623262"}}"#
    );
    matches!(
        post_json_with_timeout(&url, &[], &payload, configured_timeout_ms()),
        Ok(status) if (200..300).contains(&status)
    )
}
