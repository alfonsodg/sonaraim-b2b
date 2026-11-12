//! TDD integration tests for Milestone 12 (`sonaraim-b2b` #10, #11, #12, #13):
//! - #10: Outbound retry backoff (`post_json_with_retry`) and WhatsApp/PSP upstream forwarding

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::outbound_dispatcher::post_json_with_retry;
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
