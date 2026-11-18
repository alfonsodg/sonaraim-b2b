//! TDD tests for issue #21: POST /v1/cards/actions validates the webhook
//! timestamp window via verify_timestamped_webhook. Stale payloads are
//! rejected with an explicit error code; fresh ones pass.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::webhooks::compute_webhook_signature;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_000)
}

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

fn send_action(addr: &str, key: &str, sig: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!(
        "POST /v1/cards/actions HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {key}\r\nX-Sonaraim-Signature: {sig}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read");
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp_body.to_string())
}

#[test]
fn live_route_rejects_stale_timestamp_and_accepts_fresh() {
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:replay.example.com",
        "sk_replay_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start server");

    // Fresh: timestamp within tolerance passes.
    let fresh_ts = now_secs();
    let fresh_body = format!(
        "{{\"card_id\":\"c-1\",\"action_id\":\"approve\",\"webhook_timestamp\":{fresh_ts}}}"
    );
    let fresh_sig = compute_webhook_signature(
        &sonaraim_b2b::persistence::derive_signing_secret("did:web:replay.example.com"),
        &fresh_body,
    );
    let (st, resp) = send_action(&addr, "sk_replay_1", &fresh_sig, &fresh_body);
    assert_eq!(st, 200, "fresh webhook must pass, got {resp}");
    assert!(resp.contains("\"verified\":true"));

    // Stale: timestamp far outside the window is rejected with explicit code.
    let stale_ts = fresh_ts.saturating_sub(3_600);
    let stale_body = format!(
        "{{\"card_id\":\"c-1\",\"action_id\":\"approve\",\"webhook_timestamp\":{stale_ts}}}"
    );
    let stale_sig = compute_webhook_signature(
        &sonaraim_b2b::persistence::derive_signing_secret("did:web:replay.example.com"),
        &stale_body,
    );
    let (st, resp) = send_action(&addr, "sk_replay_1", &stale_sig, &stale_body);
    assert_eq!(st, 403, "stale webhook must be rejected, got {resp}");
    assert!(resp.contains("stale_webhook_timestamp"));

    // Missing timestamp is rejected explicitly (no silent bypass).
    let no_ts_body = r#"{"card_id":"c-1","action_id":"approve"}"#;
    let no_ts_sig = compute_webhook_signature(
        &sonaraim_b2b::persistence::derive_signing_secret("did:web:replay.example.com"),
        no_ts_body,
    );
    let (st, resp) = send_action(&addr, "sk_replay_1", &no_ts_sig, no_ts_body);
    assert_eq!(st, 400, "missing timestamp must be rejected, got {resp}");
    assert!(resp.contains("missing_webhook_timestamp"));

    drop(handle);
}
