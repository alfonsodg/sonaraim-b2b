//! Security hardening & configuration tests for `sonaraim-b2b` (#14, #15, #16, #17).

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::{B2bConfig, ConfigError};
use sonaraim_b2b::hmac_sha256::hmac_sha256;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::url_validator::validate_callback_url;
use sonaraim_b2b::webhooks::{compute_webhook_signature, encode_hex, verify_webhook_signature};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};

fn base_config() -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:18031".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 20,
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
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{hdr_str}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write request");
    let _ = stream.flush();

    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read response");
    let (head, resp_body) = resp.split_once("\r\n\r\n").unwrap_or((&resp, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp_body.to_string())
}

#[test]
fn config_requires_non_empty_b2b_storage_path() {
    let mut vars = HashMap::from([
        ("B2B_HTTP_HOST", "0.0.0.0"),
        ("B2B_HTTP_PORT", "8040"),
        ("B2B_GRPC_PORT", "8041"),
        ("B2B_NODE_RPC_URL", "http://sonaraim-node:8031"),
        ("B2B_INSTANT_CLIP_BASE_URL", "https://clip.sonaraim.io"),
        ("B2B_PROMO_RATE_LIMIT_RPS", "50"),
    ]);

    let missing = B2bConfig::from_lookup(|k| vars.get(k).map(|v| (*v).to_string()))
        .expect_err("expected missing B2B_STORAGE_PATH error");
    assert_eq!(missing, ConfigError::MissingVar("B2B_STORAGE_PATH"));

    vars.insert("B2B_STORAGE_PATH", "   ");
    let empty = B2bConfig::from_lookup(|k| vars.get(k).map(|v| (*v).to_string()))
        .expect_err("expected empty B2B_STORAGE_PATH error");
    assert_eq!(empty, ConfigError::MissingVar("B2B_STORAGE_PATH"));
}

#[test]
fn hmac_sha256_matches_rfc4231_and_rejects_sig_bypass() {
    // RFC 4231 Test Case 2: Key = "Jefe", Data = "what do ya want for nothing?"
    let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    assert_eq!(
        encode_hex(&mac),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );

    let secret = b"sk_tenant_secret_1";
    let payload = r#"{"card_id":"c-1","action_id":"approve"}"#;
    let valid_sig = compute_webhook_signature(secret, payload);
    assert!(valid_sig.starts_with("sha256="));
    assert_eq!(valid_sig.len(), 7 + 64);

    assert!(verify_webhook_signature(secret, payload, &valid_sig));
    assert!(!verify_webhook_signature(secret, payload, "sig_anything1"));
    assert!(!verify_webhook_signature(secret, payload, "sig_valid_01"));
    assert!(!verify_webhook_signature(secret, payload, "invalid"));
    assert!(!verify_webhook_signature(b"wrong_key", payload, &valid_sig));
    assert!(!verify_webhook_signature(
        secret,
        r#"{"card_id":"c-2"}"#,
        &valid_sig
    ));
}

#[test]
fn card_action_rejects_forged_sig_and_tenant_idor_over_http() {
    let mut state = B2bState::new(base_config());
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
    let (addr, handle) = start_test_server(state).expect("start server");

    // 1. Forged `sig_...` prefix bypass is rejected with 401
    let body_sig_bypass =
        r#"{"card_id":"c-1","action_id":"approve","user_signature":"sig_anything1"}"#;
    let (st, resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &[("Authorization", "Bearer sk_alpha_1")],
        body_sig_bypass,
    );
    assert_eq!(st, 401);
    assert!(resp.contains("invalid_signature"));

    // 2. Valid HMAC for alpha but targeting beta's tenant_did -> 403 Forbidden (IDOR blocked)
    let idor_body =
        r#"{"tenant_did":"did:web:beta.example.com","card_id":"c-1","action_id":"approve"}"#;
    let idor_sig = compute_webhook_signature(b"sk_alpha_1", idor_body);
    let (st, resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &[
            ("Authorization", "Bearer sk_alpha_1"),
            ("X-Sonaraim-Signature", idor_sig.as_str()),
        ],
        idor_body,
    );
    assert_eq!(st, 403);
    assert!(resp.contains("forbidden_tenant_mismatch"));

    // Verify beta's card_actions remains 0
    let (st, beta_analytics) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:beta.example.com/analytics",
        &[("Authorization", "Bearer sk_beta_1")],
        "",
    );
    assert_eq!(st, 200);
    assert!(beta_analytics.contains("\"card_actions\":0"));

    // 3. Valid HMAC with matching tenant_did succeeds and increments alpha's counter
    let ok_body =
        r#"{"tenant_did":"did:web:alpha.example.com","card_id":"c-1","action_id":"approve"}"#;
    let ok_sig = compute_webhook_signature(b"sk_alpha_1", ok_body);
    let (st, ok_resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &[
            ("Authorization", "Bearer sk_alpha_1"),
            ("X-Sonaraim-Signature", ok_sig.as_str()),
        ],
        ok_body,
    );
    assert_eq!(st, 200);
    assert!(ok_resp.contains("\"verified\":true"));

    let (st, alpha_analytics) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:alpha.example.com/analytics",
        &[("Authorization", "Bearer sk_alpha_1")],
        "",
    );
    assert_eq!(st, 200);
    assert!(alpha_analytics.contains("\"card_actions\":1"));

    drop(handle);
}

#[test]
fn callback_url_validator_blocks_ssrf_targets_and_http_in_prod() {
    assert!(validate_callback_url("https://webhooks.example.com/sonaraim", false).is_ok());

    let blocked = [
        "http://webhooks.example.com/sonaraim",
        "https://127.0.0.1/hook",
        "https://localhost/hook",
        "https://[::1]/hook",
        "https://10.0.0.1/hook",
        "https://172.16.0.1/hook",
        "https://192.168.1.10/hook",
        "https://169.254.169.254/latest/meta-data/",
        "https://0.0.0.0/hook",
        "https://224.0.0.1/hook",
        "ftp://webhooks.example.com/hook",
    ];
    for target in blocked {
        assert!(
            validate_callback_url(target, false).is_err(),
            "expected SSRF target '{target}' to be rejected"
        );
    }

    // Even in dev/test mode, metadata and RFC1918 private IPs remain blocked
    assert!(validate_callback_url("https://169.254.169.254/latest/meta-data/", true).is_err());
    assert!(validate_callback_url("http://10.0.0.1/hook", true).is_err());
    assert!(validate_callback_url("http://127.0.0.1:8080/hook", true).is_ok());
}

#[test]
fn card_action_rejects_ssrf_callback_url_with_400_without_connecting() {
    let trap_listener = TcpListener::bind("127.0.0.1:0").expect("bind trap listener");
    trap_listener
        .set_nonblocking(true)
        .expect("set nonblocking");
    let trap_addr = trap_listener.local_addr().expect("trap addr").to_string();

    let mut state = B2bState::new(base_config());
    state.register_tenant(
        "did:web:ssrf.example.com",
        "sk_ssrf_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start server");

    let ssrf_targets = [
        format!("http://{trap_addr}/steal"),
        "https://localhost/steal".to_string(),
        "https://10.0.0.5/internal".to_string(),
        "https://169.254.169.254/latest/meta-data".to_string(),
        "https://[::1]/loopback".to_string(),
    ];

    for target in ssrf_targets {
        let body = format!(r#"{{"card_id":"c-9","action_id":"pay","callback_url":"{target}"}}"#);
        let sig = compute_webhook_signature(b"sk_ssrf_1", &body);
        let (st, resp) = send_http(
            &addr,
            "POST",
            "/v1/cards/actions",
            &[
                ("Authorization", "Bearer sk_ssrf_1"),
                ("X-Sonaraim-Signature", sig.as_str()),
            ],
            &body,
        );
        assert_eq!(st, 400, "expected 400 for callback_url={target}");
        assert!(resp.contains("invalid_callback_url"));
    }

    // Confirm no outbound socket was ever opened to trap_listener
    match trap_listener.accept() {
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        other => panic!("expected no outbound connection to trap listener, got {other:?}"),
    }

    drop(handle);
}
