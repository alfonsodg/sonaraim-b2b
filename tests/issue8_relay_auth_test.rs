//! TDD tests for b2b#8 (relay-auth follow-up): the b2b-to-node relay
//! forwarder must send `Authorization: Bearer <B2B_NODE_API_TOKEN>` when the
//! token is configured, and omit the header when it is empty. Covers the
//! forwarder unit behavior plus the live `/v1/messages` route against an
//! auth-enabled mock node.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::outbound_dispatcher::forward_to_node_relay_with_retry;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn base_config(node_rpc_url: &str, node_api_token: &str) -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: node_rpc_url.to_string(),
        node_api_token: node_api_token.to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

/// Starts a mock node relay endpoint that requires the given Bearer token.
/// Returns `(base_url, received_authorization_header)`.
fn start_auth_mock_node(expected_token: &str) -> (String, Arc<Mutex<Option<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock node");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen_clone = Arc::clone(&seen);
    let expected = expected_token.to_string();
    thread::spawn(move || {
        for stream in listener.incoming().take(4).flatten() {
            handle_mock_node_conn(stream, &expected, &seen_clone);
        }
    });
    (format!("http://{addr}"), seen)
}

fn handle_mock_node_conn(mut stream: TcpStream, expected: &str, seen: &Arc<Mutex<Option<String>>>) {
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]).to_string();
    let auth = head
        .lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim().eq_ignore_ascii_case("authorization")).then(|| v.trim().to_string())
        });
    if let Ok(mut guard) = seen.lock() {
        *guard = auth.clone();
    }
    let ok = auth.as_deref() == Some(&format!("Bearer {expected}"));
    let (status, body) = if ok {
        ("202 Accepted", "ok")
    } else {
        ("401 Unauthorized", "unauthorized")
    };
    let _ = stream.write_all(
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .as_bytes(),
    );
    let _ = stream.flush();
}

fn last_seen(seen: &Arc<Mutex<Option<String>>>) -> Option<String> {
    seen.lock().ok().and_then(|g| g.clone())
}

#[test]
fn config_loads_optional_node_api_token() {
    // Token set: trimmed and preserved.
    let vars = HashMap::from([
        ("B2B_HTTP_HOST", "0.0.0.0"),
        ("B2B_HTTP_PORT", "8040"),
        ("B2B_GRPC_PORT", "8041"),
        ("B2B_NODE_RPC_URL", "http://sonaraim-node:8031"),
        ("B2B_NODE_API_TOKEN", "  node-secret-1  "),
        ("B2B_INSTANT_CLIP_BASE_URL", "https://clip.sonaraim.io/"),
        ("B2B_PROMO_RATE_LIMIT_RPS", "50"),
        ("B2B_STORAGE_PATH", "/var/lib/sonaraim-b2b/state.jsonl"),
    ]);
    let cfg = B2bConfig::from_lookup(|k| vars.get(k).map(|v| (*v).to_string()))
        .expect("expected valid B2bConfig");
    assert_eq!(cfg.node_api_token, "node-secret-1");

    // Token unset: empty (current behavior, dev-only unauthenticated nodes).
    let bare = HashMap::from([
        ("B2B_HTTP_HOST", "0.0.0.0"),
        ("B2B_HTTP_PORT", "8040"),
        ("B2B_GRPC_PORT", "8041"),
        ("B2B_NODE_RPC_URL", "http://sonaraim-node:8031"),
        ("B2B_INSTANT_CLIP_BASE_URL", "https://clip.sonaraim.io/"),
        ("B2B_PROMO_RATE_LIMIT_RPS", "50"),
        ("B2B_STORAGE_PATH", "/var/lib/sonaraim-b2b/state.jsonl"),
    ]);
    let cfg = B2bConfig::from_lookup(|k| bare.get(k).map(|v| (*v).to_string()))
        .expect("expected valid B2bConfig without token");
    assert!(cfg.node_api_token.is_empty());
}

#[test]
fn forwarder_sends_bearer_header_when_token_configured() {
    let (base_url, seen) = start_auth_mock_node("node-secret-1");
    let outcome = forward_to_node_relay_with_retry(
        &base_url,
        "node-secret-1",
        "did:web:t.example.com",
        "did:sonaraim:bob",
        1,
        "{}",
        0,
    );
    assert!(
        outcome.delivered,
        "expected 202, got {}",
        outcome.last_error
    );
    assert_eq!(last_seen(&seen).as_deref(), Some("Bearer node-secret-1"));
}

#[test]
fn forwarder_omits_bearer_header_when_token_empty() {
    // Open mock node (accepts anything) that records the request headers.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind open mock");
    let addr = listener.local_addr().expect("addr").to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = stream.write_all(
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        );
        head.lines()
            .skip(1)
            .take_while(|l| !l.trim().is_empty())
            .any(|l| l.trim().to_ascii_lowercase().starts_with("authorization:"))
    });
    let outcome = forward_to_node_relay_with_retry(
        &format!("http://{addr}"),
        "",
        "did:web:t.example.com",
        "did:sonaraim:bob",
        2,
        "{}",
        0,
    );
    assert!(
        outcome.delivered,
        "expected 202, got {}",
        outcome.last_error
    );
    assert!(
        !handle.join().expect("join"),
        "no Authorization header must be sent"
    );
}

#[test]
fn forwarder_fails_visibly_without_token_against_protected_node() {
    // Regression for the sandbox defect: empty token against a fail-closed
    // node must surface `relay upstream 401`, not a silent success.
    let (base_url, _) = start_auth_mock_node("node-secret-1");
    let outcome = forward_to_node_relay_with_retry(
        &base_url,
        "",
        "did:web:t.example.com",
        "did:sonaraim:bob",
        3,
        "{}",
        0,
    );
    assert!(!outcome.delivered);
    assert!(
        outcome.last_error.contains("401"),
        "expected 401 trace, got {}",
        outcome.last_error
    );
}

#[test]
fn messages_route_relays_with_node_bearer_against_protected_mock_node() {
    let (node_url, seen) = start_auth_mock_node("route-secret-9");
    let mut state = B2bState::new(base_config(&node_url, "route-secret-9"));
    state.register_tenant(
        "did:web:acme.example.com",
        "sk_acme_live",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start server");
    let body =
        r#"{"recipient_did":"did:sonaraim:bob","category":"TRANSACTIONAL","onboarded":true}"#;
    let req = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer sk_acme_live\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(&addr).expect("connect");
    stream.write_all(req.as_bytes()).expect("write");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read");
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    assert_eq!(status, 202, "route must accept, got {resp_body}");
    assert!(
        resp_body.contains(r#""relayed":true"#),
        "relay against the auth-enabled mock node must succeed, got {resp_body}"
    );
    // The mock node may still be processing; retry briefly for the header.
    let mut auth_seen = last_seen(&seen);
    for _ in 0..20 {
        if auth_seen.as_deref() == Some("Bearer route-secret-9") {
            break;
        }
        thread::sleep(Duration::from_millis(25));
        auth_seen = last_seen(&seen);
    }
    assert_eq!(auth_seen.as_deref(), Some("Bearer route-secret-9"));
    drop(handle);
}
