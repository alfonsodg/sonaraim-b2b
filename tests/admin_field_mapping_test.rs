//! Regression tests for the admin→B2B tenant provisioning field mapping:
//! `sonaraim-admin` `provisionB2bTenant` sends camelCase
//! (`tenantDid`/`apiKey`/`seats`), while `POST /v1/tenants` historically
//! accepted only snake_case (`tenant_did`/`api_key`/`max_seats`). The B2B
//! parser keeps snake_case primary and accepts the camelCase aliases, so
//! real admin provisioning returns 201 instead of 400.

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
        node_api_token: String::new(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn send_http(addr: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect to b2b test server");
    let req = format!(
        "POST /v1/tenants HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nX-Sonaraim-Admin-Key: kms-master-secret-99\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
fn admin_camelcase_provisioning_payload_registers_tenant() {
    let mut state = B2bState::new(sample_config());
    state.set_admin_key("kms-master-secret-99");
    let (addr, handle) = start_test_server(state).expect("start server");

    // Exact wire shape sent by sonaraim-admin provisionB2bTenant.
    let (st, resp) = send_http(
        &addr,
        r#"{"tenantDid":"did:web:admin.example.com","apiKey":"sk_admin_live","tier":"SMB_PRO","seats":10}"#,
    );
    assert_eq!(
        st, 201,
        "admin camelCase payload must register, got: {resp}"
    );
    assert!(resp.contains("did:web:admin.example.com"));
    assert!(resp.contains("\"max_seats\":10"));
    drop(handle);
}

#[test]
fn snakecase_provisioning_payload_still_registers_tenant() {
    let mut state = B2bState::new(sample_config());
    state.set_admin_key("kms-master-secret-99");
    let (addr, handle) = start_test_server(state).expect("start server");

    let (st, resp) = send_http(
        &addr,
        r#"{"tenant_did":"did:web:snake.example.com","api_key":"sk_snake_live","tier":"enterprise_sovereign","max_seats":25}"#,
    );
    assert_eq!(st, 201, "snake_case payload must keep working, got: {resp}");
    assert!(resp.contains("\"max_seats\":25"));
    drop(handle);
}
