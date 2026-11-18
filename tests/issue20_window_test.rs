//! TDD tests for issue #20: promotional rate limiter must use a real time
//! window (Redis `INCR` + `EXPIRE` / sliding window) and the in-memory
//! fallback must apply the same window semantics. Redis errors are visible.

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::persistence::{check_redis_rate_limit, last_redis_rate_limit_error};
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
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn send_promo(addr: &str, auth_key: &str, recipient: &str, sig: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let body = format!(
        "{{\"recipient_did\":\"{recipient}\",\"category\":\"PROMOTIONAL\",\"consent_token\":\"{sig}\",\"onboarded\":true}}"
    );
    let req = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {auth_key}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
fn redis_rate_limit_enforces_fixed_window_and_resets() {
    // Mock Redis speaking minimal RESP with TTL-tracked keys.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock redis");
    let redis_addr = listener.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        use std::collections::HashMap;
        let mut counts: HashMap<String, u64> = HashMap::new();
        for mut stream in listener.incoming().flatten().take(64) {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let reply = if req.contains("EXPIRE") {
                "+OK\r\n".to_string()
            } else if req.contains("INCR") {
                // Extract key: last $N bulk string.
                let parts: Vec<&str> = req.split("\r\n").collect();
                let key = parts
                    .windows(2)
                    .filter_map(|w| {
                        if w[0].starts_with('$') && w[0].len() > 1 {
                            Some(w[1].to_string())
                        } else {
                            None
                        }
                    })
                    .next_back()
                    .unwrap_or_default();
                let c = counts.entry(key).or_insert(0);
                *c += 1;
                format!(":{c}\r\n")
            } else {
                "-ERR unknown\r\n".to_string()
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });

    let redis_url = format!("redis://{redis_addr}/0");
    let key = "b2b:promo:window-test-tenant";
    // limit 2: first two pass
    assert_eq!(check_redis_rate_limit(&redis_url, key, 2), Some(true));
    assert_eq!(check_redis_rate_limit(&redis_url, key, 2), Some(true));
    // third exhausts
    assert_eq!(check_redis_rate_limit(&redis_url, key, 2), Some(false));
    // A TTL-tracked window means a different window/key passes again.
    assert_eq!(
        check_redis_rate_limit(&redis_url, "b2b:promo:other-window", 2),
        Some(true)
    );
}

#[test]
fn redis_connection_error_is_visible_not_silent() {
    last_redis_rate_limit_error(); // drain
    let res = check_redis_rate_limit("redis://127.0.0.1:9/0", "b2b:promo:x", 5);
    assert_eq!(res, None);
    let err = last_redis_rate_limit_error().expect("error must be recorded");
    assert!(!err.trim().is_empty());
}

#[test]
fn in_memory_fallback_exhausts_then_resets_after_window() {
    unsafe {
        std::env::set_var("B2B_PROMO_WINDOW_SECS", "1");
        std::env::set_var("B2B_TEST_NOW_OFFSET_SECS", "0");
    }
    let mut cfg = sample_config(2);
    let mut state = B2bState::new(cfg.clone());
    let _ = &mut cfg;
    state.redis_url = None;
    state.register_tenant(
        "did:web:promo.example.com",
        "sk_promo_1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start server");

    // limit 2 per 1s window: first two pass, third is 429.
    for i in 0..2u32 {
        let (st, _) = send_promo(
            &addr,
            "sk_promo_1",
            &format!("did:sonaraim:user{i}"),
            "sig_valid_99",
        );
        assert_eq!(st, 202, "dispatch {i} must pass inside fresh window");
    }
    let (st, body) = send_promo(&addr, "sk_promo_1", "did:sonaraim:user9", "sig_valid_99");
    assert_eq!(st, 429);
    assert!(body.contains("promo_rate_limit_exceeded"));

    // Advance the wall-clock window: the limit resets and dispatch passes again.
    unsafe {
        std::env::set_var("B2B_TEST_NOW_OFFSET_SECS", "5");
    }
    let (st, _) = send_promo(&addr, "sk_promo_1", "did:sonaraim:userA", "sig_valid_99");
    assert_eq!(
        st, 202,
        "dispatch must pass again after the window advances"
    );

    drop(handle);
    unsafe {
        std::env::remove_var("B2B_PROMO_WINDOW_SECS");
        std::env::remove_var("B2B_TEST_NOW_OFFSET_SECS");
    }
}
