//! TDD tests for issue #19: `GET /v1/tenants/{did}/analytics` and
//! `GET /v1/tenants/{did}/subscription` must require a tenant Bearer token
//! and only serve the authenticated tenant's own DID (403 otherwise).

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use std::io::{Read, Write};
use std::net::TcpStream;

fn sample_config() -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:18031".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn send_http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect to b2b test server");
    let mut hdr_str = String::new();
    for (k, v) in headers {
        hdr_str.push_str(&format!("{k}: {v}\r\n"));
    }
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n{hdr_str}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp_body.to_string())
}

fn seeded_state() -> B2bState {
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:alpha.example.com",
        "sk_alpha_1",
        SaasPlanTier::SmbPro,
        5,
    );
    state.register_tenant(
        "did:web:beta.example.com",
        "sk_beta_1",
        SaasPlanTier::SmbPro,
        5,
    );
    state
}

#[test]
fn tenant_analytics_requires_bearer_and_own_did() {
    let (addr, handle) = start_test_server(seeded_state()).expect("start server");

    // No Bearer token -> 401
    let (st, _) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:alpha.example.com/analytics",
        &[],
        "",
    );
    assert_eq!(st, 401);

    // Wrong Bearer token -> 401
    let (st, _) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:alpha.example.com/analytics",
        &[("Authorization", "Bearer wrong-key")],
        "",
    );
    assert_eq!(st, 401);

    // Authenticated as beta requesting alpha's analytics -> 403
    let (st, resp) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:alpha.example.com/analytics",
        &[("Authorization", "Bearer sk_beta_1")],
        "",
    );
    assert_eq!(st, 403);
    assert!(resp.contains("forbidden_tenant_mismatch"));

    // Authenticated as alpha requesting alpha's analytics -> 200
    let (st, resp) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:alpha.example.com/analytics",
        &[("Authorization", "Bearer sk_alpha_1")],
        "",
    );
    assert_eq!(st, 200);
    assert!(resp.contains("did:web:alpha.example.com"));

    drop(handle);
}

#[test]
fn tenant_subscription_requires_bearer_and_own_did() {
    let (addr, handle) = start_test_server(seeded_state()).expect("start server");

    // No Bearer token -> 401
    let (st, _) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:beta.example.com/subscription",
        &[],
        "",
    );
    assert_eq!(st, 401);

    // Authenticated as alpha requesting beta's subscription -> 403
    let (st, resp) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:beta.example.com/subscription",
        &[("Authorization", "Bearer sk_alpha_1")],
        "",
    );
    assert_eq!(st, 403);
    assert!(resp.contains("forbidden_tenant_mismatch"));

    // Authenticated as beta requesting beta's subscription -> 200
    let (st, resp) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:beta.example.com/subscription",
        &[("Authorization", "Bearer sk_beta_1")],
        "",
    );
    assert_eq!(st, 200);
    assert!(resp.contains("\"tier\":\"smb_pro\""));

    drop(handle);
}
