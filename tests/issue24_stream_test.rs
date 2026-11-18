//! TDD tests for issue #24: the AI agent stream listener emits real
//! incremental deltas (chunked JSON frames over the documented TCP/JSON
//! protocol) backed by a pluggable inference backend (`B2B_AI_BACKEND_URL`
//! / `B2B_AI_MODEL`); at least 3 deltas per session plus a terminal frame.

use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::http_api::B2bState;
use sonaraim_b2b::outbound_dispatcher::start_ai_stream_listener;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

fn state() -> B2bState {
    B2bState::new(B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8041,
        grpc_port: 8042,
        node_rpc_url: "http://127.0.0.1:18031".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: 50,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    })
}

fn state_with_backend(url: &str, model: &str) -> B2bState {
    let mut st = state();
    st.set_ai_backend(url, model);
    st
}

fn read_frames(stream: &mut TcpStream) -> Vec<String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read stream");
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[test]
fn stream_emits_incremental_deltas_with_pluggable_backend() {
    // Mock inference backend: deterministic 4-chunk completion.
    let backend = TcpListener::bind("127.0.0.1:0").expect("bind backend");
    let backend_addr = backend.local_addr().expect("addr").to_string();
    thread::spawn(move || {
        let (mut stream, _) = backend.accept().expect("accept backend");
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 96\r\nConnection: close\r\n\r\n{\"deltas\":[\"Order #42\",\" is ready\",\" for pickup\",\" at 10:00\"],\"model\":\"mock-1\"}",
        );
    });
    let shared = Arc::new(Mutex::new(state_with_backend(
        &format!("http://{backend_addr}/v1/complete"),
        "mock-1",
    )));
    let (stream_addr, handle) =
        start_ai_stream_listener("127.0.0.1", 0, shared).expect("start listener");

    let mut tcp = TcpStream::connect(&stream_addr).expect("connect stream");
    tcp.write_all(b"{\"agent_did\":\"did:sonaraim:agent-1\",\"prompt\":\"Summarize order #42\"}\n")
        .expect("write frame");
    let _ = tcp.shutdown(std::net::Shutdown::Write);
    let frames = read_frames(&mut tcp);

    let deltas: Vec<&String> = frames.iter().filter(|f| f.contains("\"delta\"")).collect();
    assert!(
        deltas.len() >= 3,
        "expected >=3 deltas per session, got {}: {frames:?}",
        deltas.len()
    );
    assert!(
        frames.iter().any(|f| f.contains("\"status\":\"done\"")),
        "expected a terminal done frame, got {frames:?}"
    );
    assert!(
        frames.iter().all(|f| f.contains("sonaraim.ai.v1")),
        "every frame must carry the protocol marker: {frames:?}"
    );
    drop(handle);
}

#[test]
fn stream_without_backend_still_streams_incremental_echo_deltas() {
    let shared = Arc::new(Mutex::new(state()));
    let (stream_addr, handle) =
        start_ai_stream_listener("127.0.0.1", 0, shared).expect("start listener");

    let mut tcp = TcpStream::connect(&stream_addr).expect("connect stream");
    tcp.write_all(b"{\"agent_did\":\"did:sonaraim:agent-9\",\"prompt\":\"Hello world\"}\n")
        .expect("write frame");
    let _ = tcp.shutdown(std::net::Shutdown::Write);
    let frames = read_frames(&mut tcp);

    let deltas: Vec<&String> = frames.iter().filter(|f| f.contains("\"delta\"")).collect();
    assert!(
        deltas.len() >= 3,
        "fallback must also stream >=3 deltas, got {frames:?}"
    );
    assert!(frames.iter().any(|f| f.contains("\"status\":\"done\"")));
    drop(handle);
}
