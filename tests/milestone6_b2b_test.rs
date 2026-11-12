//! TDD integration tests for Milestone 6 & 8 (`sonaraim-b2b` #7):
//! - #7: Durable JSONL tenant/consent store, dynamic `POST /v1/tenants` admin provisioning,
//!   and Redis RESP rate limiter.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::persistence::{check_redis_rate_limit, load_snapshot, save_snapshot};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

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
fn dynamic_tenant_provisioning_and_durable_jsonl_snapshot_persistence_work() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let snap_path = std::env::temp_dir()
        .join(format!("sonaraim_b2b_m6_{nanos}.jsonl"))
        .to_string_lossy()
        .to_string();

    let mut state = B2bState::new(sample_config(5));
    state.set_admin_key("kms-master-secret-99");
    state.set_storage_path(&snap_path);

    let (addr, handle) = start_test_server(state).expect("start server");

    // 1. Unauthorized POST /v1/tenants without valid X-Sonaraim-Admin-Key -> 401
    let (st, _) = send_http(
        &addr,
        "POST",
        "/v1/tenants",
        &[("X-Sonaraim-Admin-Key", "wrong-key")],
        r#"{"tenant_did":"did:web:acme.example.com","api_key":"sk_acme_live","tier":"enterprise_sovereign","max_seats":25}"#,
    );
    assert_eq!(st, 401);

    // 2. Authorized POST /v1/tenants -> 201 Created and persisted to disk
    let (st, reg_resp) = send_http(
        &addr,
        "POST",
        "/v1/tenants",
        &[("X-Sonaraim-Admin-Key", "kms-master-secret-99")],
        r#"{"tenant_did":"did:web:acme.example.com","api_key":"sk_acme_live","tier":"enterprise_sovereign","max_seats":25}"#,
    );
    assert_eq!(st, 201);
    assert!(reg_resp.contains("did:web:acme.example.com"));
    assert!(reg_resp.contains("enterprise_sovereign"));

    // 3. Newly provisioned tenant immediately dispatches messages and revokes consent
    let acme_auth = [("Authorization", "Bearer sk_acme_live")];
    let (st, msg_resp) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &acme_auth,
        r#"{"recipient_did":"did:sonaraim:bob","category":"TRANSACTIONAL","onboarded":false}"#,
    );
    assert_eq!(st, 202);
    assert!(msg_resp.contains("SMART_FALLBACK_INSTANT_CLIP"));

    let (st, _) = send_http(
        &addr,
        "POST",
        "/v1/consents/revoke",
        &acme_auth,
        r#"{"user_did":"did:sonaraim:bob"}"#,
    );
    assert_eq!(st, 200);
    drop(handle);

    // 4. Reload snapshot into a fresh B2bState and verify tenant + revoked consent survive restart
    let mut restored = B2bState::new(sample_config(5));
    load_snapshot(&mut restored, &snap_path).expect("reload snapshot");
    save_snapshot(&restored, &snap_path).expect("re-save snapshot");

    let (addr2, handle2) = start_test_server(restored).expect("start restored server");
    let (st, sub_resp) = send_http(
        &addr2,
        "GET",
        "/v1/tenants/did:web:acme.example.com/subscription",
        &acme_auth,
        "",
    );
    assert_eq!(st, 200);
    assert!(sub_resp.contains("\"tier\":\"enterprise_sovereign\""));
    assert!(sub_resp.contains("\"max_seats\":25"));

    let (st, promo_resp) = send_http(
        &addr2,
        "POST",
        "/v1/messages",
        &acme_auth,
        r#"{"recipient_did":"did:sonaraim:bob","category":"PROMOTIONAL","consent_token":"sig_valid_99","onboarded":true}"#,
    );
    assert_eq!(st, 403);
    assert!(promo_resp.contains("consent_revoked"));

    let _ = std::fs::remove_file(&snap_path);
    drop(handle2);
}

#[test]
fn redis_resp_rate_limiter_enforces_promotional_rps_over_tcp() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis");
    let redis_addr = listener.local_addr().expect("local addr").to_string();

    thread::spawn(move || {
        let mut count = 0u64;
        for mut stream in listener.incoming().flatten().take(3) {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            count += 1;
            let reply = format!(":{count}\r\n");
            let _ = stream.write_all(reply.as_bytes());
        }
    });

    let redis_url = format!("redis://{redis_addr}/0");
    assert_eq!(
        check_redis_rate_limit(&redis_url, "b2b:promo:did:web:acme.example.com", 2),
        Some(true)
    );
    assert_eq!(
        check_redis_rate_limit(&redis_url, "b2b:promo:did:web:acme.example.com", 2),
        Some(true)
    );
    assert_eq!(
        check_redis_rate_limit(&redis_url, "b2b:promo:did:web:acme.example.com", 2),
        Some(false)
    );

    let mut s = B2bState::new(sample_config(2));
    s.register_tenant("did:web:t.example.com", "k", SaasPlanTier::Free, 1);
    assert_eq!(s.tenants.len(), 1);
}
