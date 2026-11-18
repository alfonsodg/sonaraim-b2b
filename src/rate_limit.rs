//! Fixed-window promotional rate limiter over Redis RESP with an
//! observable error slot, plus the configured window helpers.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Returns `Some(count <= limit)` on success, recording failures for operators.
///
/// The counter key carries a 1-second TTL set via `EXPIRE` so the window
/// resets in real time instead of accumulating forever. On any I/O or RESP
/// error the failure is recorded (retrievable via
/// [`last_redis_rate_limit_error`]) and `None` is returned so callers can
/// fail open while remaining observable.
pub fn check_redis_rate_limit(redis_url: &str, key: &str, limit: u64) -> Option<bool> {
    check_redis_rate_limit_with_window(redis_url, key, limit, 1)
}

/// Records the most recent Redis rate-limiter failure for operator visibility.
static REDIS_RATE_LIMIT_ERROR: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Returns and clears the last recorded Redis rate-limiter error, if any.
pub fn last_redis_rate_limit_error() -> Option<String> {
    REDIS_RATE_LIMIT_ERROR.lock().ok()?.take()
}

/// Records a Redis rate-limiter failure message for operator visibility.
fn record_redis_rate_limit_error(message: String) {
    if let Ok(mut slot) = REDIS_RATE_LIMIT_ERROR.lock() {
        *slot = Some(message);
    }
    eprintln!(
        "{{\"level\":\"WARN\",\"service\":\"sonaraim-b2b\",\"component\":\"promo_rate_limiter\",\"error\":\"redis_unavailable\"}}"
    );
}

/// Executes a Redis RESP `INCR` + `EXPIRE key window_secs` fixed-window check.
pub fn check_redis_rate_limit_with_window(
    redis_url: &str,
    key: &str,
    limit: u64,
    window_secs: u64,
) -> Option<bool> {
    let outcome = check_redis_once(redis_url, key, limit, window_secs.max(1));
    if outcome.is_none() {
        record_redis_rate_limit_error(format!("redis_unavailable key={key}"));
    }
    outcome
}

/// Single-attempt `INCR` + `EXPIRE` round trip against a `redis://` endpoint.
fn check_redis_once(redis_url: &str, key: &str, limit: u64, window_secs: u64) -> Option<bool> {
    let stripped = redis_url
        .trim()
        .strip_prefix("redis://")
        .unwrap_or(redis_url.trim());
    let host_port = stripped.split('/').next()?;
    let addr: SocketAddr = host_port.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(150)));
    let incr = format!("*2\r\n$4\r\nINCR\r\n${}\r\n{key}\r\n", key.len());
    stream.write_all(incr.as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).ok()?;
    let count: u64 = String::from_utf8_lossy(&buf[..n])
        .trim_start_matches(':')
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    // Best-effort TTL so the fixed window resets; a failed EXPIRE still
    // counts this attempt but is reported via the error slot below.
    let expire = format!(
        "*3\r\n$6\r\nEXPIRE\r\n${}\r\n{key}\r\n${}\r\n{window_secs}\r\n",
        key.len(),
        window_secs.to_string().len()
    );
    if stream.write_all(expire.as_bytes()).is_err() {
        return None;
    }
    let _ = stream.flush();
    let mut ack = [0u8; 64];
    let _ = stream.read(&mut ack);
    Some(count <= limit)
}

/// Returns the test-only wall-clock offset in seconds (`B2B_TEST_NOW_OFFSET_SECS`).
pub(crate) fn test_now_offset_secs() -> u64 {
    std::env::var("B2B_TEST_NOW_OFFSET_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

/// Returns the configured promotional rate-limit window in seconds
/// (`B2B_PROMO_WINDOW_SECS`, default `1`).
pub fn promo_window_secs() -> u64 {
    std::env::var("B2B_PROMO_WINDOW_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(1)
}
