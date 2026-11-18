//! TDD remediation tests for b2b#22 (re-save kills credentials) and b2b#23
//! (delivery queue not wired to live routes).
//!
//! These tests FAIL against the pre-fix code:
//! - re-save (`load` -> `save` -> `load`) destroys Bearer auth because
//!   `register_hashed_tenant` leaves `api_key: ""` and `save_snapshot`
//!   persists `hash_credential("")`.
//! - legacy cleartext snapshots (`api_key`, no checksum) are rejected.
//! - `DeliveryQueue::reload` drops bodies/headers.
//! - live routes reply synchronously instead of enqueueing with `delivery_id`.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::delivery_queue::{DeliveryJob, DeliveryQueue};
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::persistence::{hash_credential, load_snapshot, save_snapshot};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{SystemTime, UNIX_EPOCH};

const LOOPBACK: &str = "127.0.0.1";
const DARK_PORT: u16 = 9;

fn dark_host() -> String {
    format!("{LOOPBACK}:{DARK_PORT}")
}

fn dark_url(path: &str) -> String {
    format!("http://{}{}", dark_host(), path)
}

fn sample_config() -> B2bConfig {
    B2bConfig {
        http_host: LOOPBACK.to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: format!("http://{}:18031", LOOPBACK),
        node_api_token: String::new(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn snap_path(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir()
        .join(format!(
            "sonaraim_b2b_rem_{tag}_{nanos}_{}.jsonl",
            std::process::id()
        ))
        .to_string_lossy()
        .to_string()
}

fn send_http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let mut extra = String::new();
    for (k, v) in headers {
        extra.push_str(&format!("{k}: {v}\r\n"));
    }
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
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
    (status, resp_body.to_string())
}

#[test]
fn resave_is_idempotent_bearer_auth_survives() {
    let path = snap_path("resave");
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:resave.example.com",
        "sk_resave_secret",
        SaasPlanTier::SmbPro,
        5,
    );
    let expected_hash = hash_credential("sk_resave_secret");
    save_snapshot(&state, &path).expect("save");

    // First reload...
    let mut restored = B2bState::new(sample_config());
    load_snapshot(&mut restored, &path).expect("reload");
    assert_eq!(
        restored.api_keys.get(&expected_hash).map(String::as_str),
        Some("did:web:resave.example.com"),
        "hashed lookup must survive first load"
    );

    // ...then re-save WITHOUT cleartext key, and auth must still work.
    save_snapshot(&restored, &path).expect("re-save");
    let raw = std::fs::read_to_string(&path).expect("read re-saved");
    assert!(
        !raw.contains(&hash_credential("")),
        "re-save must NOT persist hash of empty credential"
    );

    let mut reloaded = B2bState::new(sample_config());
    load_snapshot(&mut reloaded, &path).expect("reload after re-save");
    assert_eq!(
        reloaded.api_keys.get(&expected_hash).map(String::as_str),
        Some("did:web:resave.example.com"),
        "Bearer auth must survive load->save->reload"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn legacy_cleartext_snapshot_migrates_to_hashed() {
    let path = snap_path("legacy");
    let tenant_line = r#"{"kind":"tenant","did":"did:web:legacy.example.com","api_key":"sk_legacy_1","tier":"smb_pro","max_seats":5,"promo_requests":2,"promo_window_start":0,"promo_window_count":0,"card_actions":1,"messages_sent":3}"#;
    std::fs::write(&path, format!("{tenant_line}\n")).expect("write legacy snapshot");

    let mut restored = B2bState::new(sample_config());
    load_snapshot(&mut restored, &path).expect("legacy snapshot must migrate, not fail");

    // Post-migration, the hashed lookup must work.
    let expected_hash = hash_credential("sk_legacy_1");
    assert_eq!(
        restored.api_keys.get(&expected_hash).map(String::as_str),
        Some("did:web:legacy.example.com"),
        "legacy cleartext key must hash-migrate into the lookup"
    );

    // The migrated state must re-save in the NEW hashed format with checksum.
    let migrated = snap_path("legacy_migrated");
    save_snapshot(&restored, &migrated).expect("save migrated");
    let raw = std::fs::read_to_string(&migrated).expect("read migrated");
    assert!(
        !raw.contains("sk_legacy_1"),
        "migrated snapshot must not leak cleartext key"
    );
    assert!(raw.contains("api_key_sha256"));
    assert!(raw.contains("checksum"));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&migrated);
}

#[test]
fn card_action_accepts_legacy_and_derived_signing_secrets() {
    // Dual-accept window (b2b#22): the pre-derivation legacy payload-only
    // FNV signer and the current derived-HMAC signer must BOTH verify, so
    // the secret upgrade does not brick external signers. See docs/API.md.
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:dual.example.com",
        "sk_dual_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start server");
    let auth = [("Authorization", "Bearer sk_dual_1")];
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_100);

    // Legacy path: payload-only FNV signature over the exact body, header-sent.
    let body_legacy =
        format!("{{\"card_id\":\"c-dual\",\"action_id\":\"pay\",\"webhook_timestamp\":{ts}}}");
    let legacy_sig = sonaraim_b2b::webhooks::compute_legacy_webhook_signature(&body_legacy);
    let headers_legacy = [
        ("Authorization", "Bearer sk_dual_1"),
        ("X-Sonaraim-Signature", legacy_sig.as_str()),
    ];
    let (st, resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &headers_legacy,
        &body_legacy,
    );
    assert_eq!(st, 200, "legacy signer must be accepted: {resp}");
    assert!(resp.contains("\"verified\":true"));

    // Modern path: derived-HMAC signature over the exact body, header-sent.
    let body_modern =
        format!("{{\"card_id\":\"c-dual2\",\"action_id\":\"pay\",\"webhook_timestamp\":{ts}}}");
    let modern_sig = sonaraim_b2b::webhooks::compute_webhook_signature(
        &sonaraim_b2b::persistence::derive_signing_secret("did:web:dual.example.com"),
        &body_modern,
    );
    let headers_modern = [
        ("Authorization", "Bearer sk_dual_1"),
        ("X-Sonaraim-Signature", modern_sig.as_str()),
    ];
    let (st, resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &headers_modern,
        &body_modern,
    );
    assert_eq!(st, 200, "derived signer must be accepted: {resp}");
    assert!(resp.contains("\"verified\":true"));

    // Forgery control: garbage signatures still fail.
    let (st, _) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &auth,
        &format!(
            "{{\"card_id\":\"c-dual\",\"action_id\":\"pay\",\"webhook_timestamp\":{ts},\"user_signature\":\"sig_forged\"}}"
        ),
    );
    assert_eq!(st, 401);
    drop(handle);
}

#[test]
fn queue_reload_preserves_bodies_and_headers() {
    let dir = std::env::temp_dir().join(format!(
        "sonaraim_b2b_rem_q_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let queue_path = dir.join("queue.jsonl").to_string_lossy().to_string();
    let dlq_path = dir.join("dead-letter.jsonl").to_string_lossy().to_string();

    let mut queue = DeliveryQueue::new(&queue_path, &dlq_path, 5);
    queue.enqueue(DeliveryJob {
        id: "job-body-1".to_string(),
        url: dark_url("/hook"),
        headers: vec![("X-Sonaraim-Signature".to_string(), "sha256=abc".to_string())],
        body: r#"{"hello":"world"}"#.to_string(),
        attempts: 0,
    });

    // The queue file must round-trip bodies and headers (hex-encoded so
    // arbitrary JSON survives the JSONL line format): decode and compare.
    let raw = std::fs::read_to_string(&queue_path).expect("read queue file");
    let body_hex = sonaraim_b2b::webhooks::json_str(&raw, "body_hex").expect("body_hex");
    let body =
        String::from_utf8(sonaraim_b2b::webhooks::decode_hex(&body_hex).expect("decode body"))
            .expect("utf8 body");
    assert_eq!(body, r#"{"hello":"world"}"#);
    let headers_hex = sonaraim_b2b::webhooks::json_str(&raw, "headers_hex").expect("headers_hex");
    assert!(
        headers_hex.contains(&sonaraim_b2b::webhooks::encode_hex(b"X-Sonaraim-Signature")),
        "queue file must persist headers, got {raw}"
    );

    // ...and a fresh queue over the same file must restore the full job.
    let mut restored = DeliveryQueue::new(&queue_path, &dlq_path, 5);
    assert_eq!(restored.pending(), 1, "pending job must survive reload");
    // Draining against the dark port must re-queue (not drop) since the
    // body/headers survived: pending stays 1 after one attempt with attempts=1.
    restored.drain_once(25);
    assert_eq!(restored.pending(), 1, "failed job must stay pending");
    let raw2 = std::fs::read_to_string(&queue_path).expect("read queue file again");
    let body_hex2 = sonaraim_b2b::webhooks::json_str(&raw2, "body_hex").expect("body_hex");
    let body2 =
        String::from_utf8(sonaraim_b2b::webhooks::decode_hex(&body_hex2).expect("decode body"))
            .expect("utf8 body");
    assert_eq!(
        body2, r#"{"hello":"world"}"#,
        "re-persisted queue must keep bodies"
    );
}

#[test]
fn live_message_route_enqueues_and_returns_delivery_id() {
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:liveq.example.com",
        "sk_liveq_1",
        SaasPlanTier::SmbPro,
        5,
    );
    state.config.allow_insecure_callbacks = true;
    let (addr, handle) = start_test_server(state).expect("start server");
    let (st, resp) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &[("Authorization", "Bearer sk_liveq_1")],
        r#"{"recipient_did":"did:sonaraim:bob","category":"TRANSACTIONAL","onboarded":false}"#,
    );
    assert_eq!(st, 202, "live route must accept, got {resp}");
    assert!(
        resp.contains("delivery_id"),
        "live route must return a delivery id, got {resp}"
    );
    drop(handle);
}
