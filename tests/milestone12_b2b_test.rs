//! TDD integration tests for Milestone 12 (`sonaraim-b2b` #10, #11, #12, #13):
//! - #10: Outbound retry backoff (`post_json_with_retry`) and WhatsApp/PSP upstream forwarding

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::outbound_dispatcher::{forward_to_node_relay, post_json_with_retry};
use sonaraim_b2b::webhooks::{
    compute_webhook_signature, encode_hex, json_str, verify_timestamped_webhook,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

fn sample_config(promo_rps: u32) -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:18031".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: promo_rps,
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

#[test]
fn outbound_retry_backoff_and_upstream_whatsapp_psp_forwarding_work() {
    // Mock server that returns 503 on attempt 1, then 200 OK on attempt 2, and 200 OK on attempt 3
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock upstream");
    let upstream_addr = listener.local_addr().expect("upstream addr").to_string();
    let upstream_handle = thread::spawn(move || {
        let mut attempts = 0usize;
        for mut stream in listener.incoming().flatten().take(3) {
            attempts += 1;
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            if attempts == 1 {
                let _ = stream.write_all(
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            } else {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                );
            }
        }
        attempts
    });

    let retry_url = format!("http://{upstream_addr}/v1/psp/intents");
    let status = post_json_with_retry(&retry_url, &[], r#"{"ping":1}"#, 200, 2)
        .expect("retry should recover after 503");
    assert_eq!(status, 200);

    let mut state = B2bState::new(sample_config(10));
    state.register_tenant(
        "did:web:pay.example.com",
        "sk_pay_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (b2b_addr, b2b_handle) = start_test_server(state).expect("start b2b");
    let auth = [("Authorization", "Bearer sk_pay_1")];

    let pay_body = format!(
        r#"{{"provider":"stripe","amount_minor":5000,"take_rate_bps":100,"upstream_url":"{retry_url}"}}"#
    );
    let (st, pay_resp) = send_http(&b2b_addr, "POST", "/v1/payments/intents", &auth, &pay_body);
    assert_eq!(st, 201);
    assert!(pay_resp.contains("\"upstream_synced\":true"));

    let total_attempts = upstream_handle.join().expect("join upstream");
    assert_eq!(total_attempts, 3);
    drop(b2b_handle);
}

#[test]
fn webhook_timestamp_replay_window_and_tenant_key_rotation_work() {
    let payload = r#"{"card_id":"c-99","action_id":"approve"}"#;
    let sig = compute_webhook_signature(payload);
    // Fresh timestamp within 300s window passes
    assert!(verify_timestamped_webhook(
        b"sec",
        1_700_000_100,
        1_700_000_200,
        300,
        payload.as_bytes(),
        &sig
    ));
    // Stale timestamp outside 300s window is rejected
    assert!(!verify_timestamped_webhook(
        b"sec",
        1_700_000_000,
        1_700_000_600,
        300,
        payload.as_bytes(),
        &sig
    ));

    let mut state = B2bState::new(sample_config(10));
    state.register_tenant(
        "did:web:rotate.example.com",
        "sk_old_key_1",
        SaasPlanTier::EnterpriseSovereign,
        20,
    );
    let (addr, handle) = start_test_server(state).expect("start server");

    // Rotate key using current Bearer token
    let (st, rot_resp) = send_http(
        &addr,
        "POST",
        "/v1/tenants/rotate-key",
        &[("Authorization", "Bearer sk_old_key_1")],
        r#"{"new_api_key":"sk_new_key_2"}"#,
    );
    assert_eq!(st, 200);
    assert!(rot_resp.contains("\"status\":\"rotated\""));

    // Old key is immediately revoked -> 401
    let (st_old, _) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &[("Authorization", "Bearer sk_old_key_1")],
        r#"{"recipient_did":"did:sonaraim:alice","category":"TRANSACTIONAL","onboarded":false}"#,
    );
    assert_eq!(st_old, 401);

    // New key succeeds -> 202
    let (st_new, _) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &[("Authorization", "Bearer sk_new_key_2")],
        r#"{"recipient_did":"did:sonaraim:alice","category":"TRANSACTIONAL","onboarded":false}"#,
    );
    assert_eq!(st_new, 202);

    drop(handle);
}

#[test]
fn relay_forwarding_encodes_real_message_card_json_in_payload_hex() {
    let node_listener = TcpListener::bind("127.0.0.1:0").expect("bind mock node");
    let node_addr = node_listener.local_addr().expect("node addr").to_string();
    let node_handle = thread::spawn(move || {
        let (mut stream, _) = node_listener.accept().expect("accept node relay");
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).expect("read relay req");
        let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
        let reply = "HTTP/1.1 202 Accepted\r\nContent-Length: 23\r\nConnection: close\r\n\r\n{\"wake_up_token\":\"wk2\"}";
        stream
            .write_all(reply.as_bytes())
            .expect("write relay resp");
        req_str
    });

    let mut cfg = sample_config(10);
    cfg.node_rpc_url = format!("http://{node_addr}");
    let mut state = B2bState::new(cfg);
    state.register_tenant(
        "did:web:bakery.example.com",
        "sk_bakery_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (b2b_addr, b2b_handle) = start_test_server(state).expect("start b2b");

    let msg_json = r#"{"recipient_did":"did:sonaraim:elena","category":"TRANSACTIONAL","onboarded":true,"title":"Order #881 Ready","action_label":"Confirm Pickup"}"#;
    let (st, resp) = send_http(
        &b2b_addr,
        "POST",
        "/v1/messages",
        &[("Authorization", "Bearer sk_bakery_1")],
        msg_json,
    );
    assert_eq!(st, 202);
    assert!(resp.contains("\"relayed\":true"));

    let raw_req = node_handle.join().expect("join node mock");
    let (_, req_body) = raw_req.split_once("\r\n\r\n").unwrap_or(("", ""));
    assert_eq!(
        json_str(req_body, "tenant_did"),
        Some("did:web:bakery.example.com".to_string())
    );
    let payload_hex = json_str(req_body, "payload_hex").expect("payload_hex field");
    assert_ne!(payload_hex, "736f6e617261696d5f623262");
    let expected_sub = encode_hex(b"Order #881 Ready");
    assert!(payload_hex.contains(&expected_sub));

    // Also test direct 5-arg forward_to_node_relay call against offline port returns false cleanly
    assert!(!forward_to_node_relay(
        "http://127.0.0.1:9",
        "did:web:bakery.example.com",
        "did:sonaraim:elena",
        1,
        msg_json
    ));

    drop(b2b_handle);
}

#[test]
fn whatsapp_compat_normalizes_e164_and_relays_to_node() {
    let node_listener = TcpListener::bind("127.0.0.1:0").expect("bind mock node");
    let node_addr = node_listener.local_addr().expect("node addr").to_string();
    let node_handle = thread::spawn(move || {
        let (mut stream, _) = node_listener.accept().expect("accept node relay");
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).expect("read relay req");
        let req_str = String::from_utf8_lossy(&buf[..n]).to_string();
        let reply =
            "HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"status\":\"ok\"}";
        stream
            .write_all(reply.as_bytes())
            .expect("write relay resp");
        req_str
    });

    let mut cfg = sample_config(11);
    cfg.node_rpc_url = format!("http://{node_addr}");
    let mut state = B2bState::new(cfg);
    state.register_tenant(
        "did:web:clinic.example.com",
        "sk_clinic_1",
        SaasPlanTier::SmbPro,
        10,
    );
    let (b2b_addr, b2b_handle) = start_test_server(state).expect("start b2b");

    let wa_payload = r#"{"to":"+51999888777","template":"dental_reminder","params":["10:00 AM"]}"#;
    let (st, resp) = send_http(
        &b2b_addr,
        "POST",
        "/v1/whatsapp-compat/555123/messages",
        &[("Authorization", "Bearer sk_clinic_1")],
        wa_payload,
    );
    assert_eq!(st, 200);
    assert!(resp.contains("\"relayed\":true"));
    assert!(resp.contains("\"recipient\":\"did:sonaraim:+51999888777\""));

    let raw_req = node_handle.join().expect("join mock node");
    let (_, req_body) = raw_req.split_once("\r\n\r\n").unwrap_or(("", ""));
    assert_eq!(
        json_str(req_body, "recipient_did"),
        Some("did:sonaraim:+51999888777".to_string())
    );
    assert_eq!(
        json_str(req_body, "recipient_mailbox_id"),
        Some("did:sonaraim:+51999888777".to_string())
    );
    assert_eq!(
        json_str(req_body, "tenant_did"),
        Some("did:web:clinic.example.com".to_string())
    );

    let payload_hex = json_str(req_body, "payload_hex").expect("payload_hex");
    let expected_title_hex = encode_hex(b"dental_reminder");
    assert!(payload_hex.contains(&expected_title_hex));

    drop(b2b_handle);
}
