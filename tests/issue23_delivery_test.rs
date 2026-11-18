//! TDD tests for issue #23: async webhook delivery queue with durable
//! backlog, exponential backoff with jitter, and a visible dead-letter
//! record. The relay forwarder must report real failures (no silent
//! `relayed:false` in a 202).

use sonaraim_b2b::billing::SaasPlanTier;
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::delivery_queue::{DeliveryJob, DeliveryQueue};
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::outbound_dispatcher::forward_to_node_relay_with_retry;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

fn sample_config() -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:9".to_string(),
        node_api_token: String::new(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

#[test]
fn queue_retries_with_backoff_then_dead_letters_visibly() {
    let dir = std::env::temp_dir().join(format!(
        "sonaraim_b2b_23_dlq_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let queue_path = dir.join("queue.jsonl").to_string_lossy().to_string();
    let dlq_path = dir.join("dead-letter.jsonl").to_string_lossy().to_string();

    let mut queue = DeliveryQueue::new(&queue_path, &dlq_path, 3);
    queue.enqueue(DeliveryJob {
        id: "job-1".to_string(),
        url: "http://127.0.0.1:9/unreachable-hook".to_string(),
        headers: vec![],
        body: r#"{"ping":1}"#.to_string(),
        attempts: 0,
    });

    // Backoff grows exponentially with jitter: attempt delays are ordered.
    let d0 = DeliveryQueue::backoff_ms(0, 100);
    let d1 = DeliveryQueue::backoff_ms(1, 100);
    let d2 = DeliveryQueue::backoff_ms(2, 100);
    assert!(d0 <= 150, "first backoff ~base, got {d0}");
    assert!(d1 >= 150, "backoff must grow, got {d1}");
    assert!(d2 >= 300, "backoff must keep growing, got {d2}");

    // Drain against an unreachable target: retries exhaust, then the job
    // lands in the visible DLQ (one drain pass == one attempt per job).
    for _ in 0..5 {
        if queue.pending() == 0 {
            break;
        }
        queue.drain_once(25);
    }
    assert_eq!(queue.pending(), 0);
    assert_eq!(queue.dead_lettered(), 1);
    let dlq_raw = std::fs::read_to_string(&dlq_path).expect("read dlq");
    assert!(dlq_raw.contains("job-1"));
    assert!(dlq_raw.contains("unreachable-hook"));
}

#[test]
fn relay_forwarder_with_retry_reports_real_outcome() {
    // Unreachable relay: retry variant reports failure explicitly.
    let fail = forward_to_node_relay_with_retry(
        "http://127.0.0.1:9",
        "",
        "did:web:t.example.com",
        "did:sonaraim:bob",
        7,
        "{}",
        0,
    );
    assert!(!fail.delivered);
    assert!(fail.attempts >= 1);
    assert!(!fail.last_error.trim().is_empty());

    // Live relay mock accepting the envelope: delivered with attempts >= 1.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock relay");
    let relay_addr = listener.local_addr().expect("addr").to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(
            b"HTTP/1.1 202 Accepted\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        );
    });
    let ok = forward_to_node_relay_with_retry(
        &format!("http://{relay_addr}"),
        "",
        "did:web:t.example.com",
        "did:sonaraim:bob",
        8,
        "{}",
        0,
    );
    assert!(ok.delivered);
    assert!(ok.attempts >= 1);
    handle.join().expect("join relay mock");
}

#[test]
fn card_action_delivery_failure_is_visible_not_silent_202() {
    // Callback URL that refuses connections: route must surface the failure
    // explicitly instead of a silent `relayed:false` inside a 200.
    let mut state = B2bState::new(sample_config());
    state.register_tenant(
        "did:web:hook.example.com",
        "sk_hook_1",
        SaasPlanTier::SmbPro,
        5,
    );
    state.config.allow_insecure_callbacks = true;
    let (addr, handle) = start_test_server(state).expect("start server");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(1_700_000_100);
    let body = format!(
        "{{\"card_id\":\"c-dlq\",\"action_id\":\"pay\",\"callback_url\":\"http://127.0.0.1:9/dead\",\"webhook_timestamp\":{ts}}}"
    );
    let sig = sonaraim_b2b::webhooks::compute_webhook_signature(
        &sonaraim_b2b::persistence::derive_signing_secret("did:web:hook.example.com"),
        &body,
    );
    let mut stream = TcpStream::connect(&addr).expect("connect");
    let req = format!(
        "POST /v1/cards/actions HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer sk_hook_1\r\nX-Sonaraim-Signature: {sig}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
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
    assert_eq!(status, 200);
    assert!(
        resp_body.contains("webhook_delivered"),
        "delivery outcome must stay visible, got {resp_body}"
    );
    drop(handle);
}
