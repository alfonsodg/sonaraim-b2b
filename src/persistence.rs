//! Durable JSONL snapshot persistence for tenants and revoked consents, dynamic
//! `POST /v1/tenants` admin provisioning handler, and Redis RESP rate limiter.

use crate::billing::SaasPlanTier;
use crate::consent::{ConsentError, ConsentGate, ConsentToken, MessageCategory};
use crate::http_api::B2bState;
use crate::webhooks::{HttpRequest, json_str, json_u64};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Parses a [`SaasPlanTier`] identifier (case-insensitive), defaulting to [`SaasPlanTier::SmbPro`].
pub fn parse_tier(raw: &str) -> SaasPlanTier {
    match raw.trim().to_ascii_lowercase().as_str() {
        "free" => SaasPlanTier::Free,
        "enterprise_sovereign" | "enterprise" => SaasPlanTier::EnterpriseSovereign,
        _ => SaasPlanTier::SmbPro,
    }
}

/// Saves all registered tenants and revoked consent keys in [`B2bState`] to a JSONL file.
pub fn save_snapshot(state: &B2bState, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    for t in state.tenants.values() {
        out.push_str(&format!(
            r#"{{"kind":"tenant","did":"{}","api_key":"{}","tier":"{}","max_seats":{},"promo_requests":{},"promo_window_start":{},"promo_window_count":{},"card_actions":{},"messages_sent":{}}}"#,
            t.did, t.api_key, t.tier.as_str(), t.max_seats, t.promo_requests, t.promo_window_start, t.promo_window_count, t.card_actions, t.messages_sent
        ));
        out.push('\n');
    }
    for key in &state.revoked_consents {
        out.push_str(&format!(r#"{{"kind":"revoked","key":"{key}"}}"#));
        out.push('\n');
    }
    fs::write(path, out)
}

/// Loads registered tenants and revoked consents from a JSONL snapshot file into [`B2bState`].
pub fn load_snapshot(state: &mut B2bState, path: &str) -> io::Result<()> {
    if !Path::new(path).exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path)?;
    for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match json_str(line, "kind").as_deref() {
            Some("tenant") => {
                let did = json_str(line, "did").unwrap_or_default();
                let api_key = json_str(line, "api_key").unwrap_or_default();
                let tier = parse_tier(&json_str(line, "tier").unwrap_or_default());
                let max_seats = json_u64(line, "max_seats").unwrap_or(10) as u32;
                if !did.is_empty() && !api_key.is_empty() {
                    state.register_tenant(&did, &api_key, tier, max_seats);
                    if let Some(rec) = state.tenants.get_mut(&did) {
                        rec.promo_requests = json_u64(line, "promo_requests").unwrap_or(0);
                        rec.promo_window_start = json_u64(line, "promo_window_start").unwrap_or(0);
                        rec.promo_window_count = json_u64(line, "promo_window_count").unwrap_or(0);
                        rec.card_actions = json_u64(line, "card_actions").unwrap_or(0);
                        rec.messages_sent = json_u64(line, "messages_sent").unwrap_or(0);
                    }
                }
            }
            Some("revoked") => {
                if let Some(k) = json_str(line, "key")
                    && !k.is_empty()
                {
                    state.revoked_consents.insert(k);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Persists [`B2bState`] to its configured `storage_path` if one is set.
pub fn persist_if_configured(state: &B2bState) {
    if let Some(ref path) = state.storage_path {
        let _ = save_snapshot(state, path);
    }
}

/// Handles `POST /v1/tenants` admin provisioning protected by `X-Sonaraim-Admin-Key`.
pub fn handle_register_tenant(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return (
            500,
            "Internal Server Error",
            r#"{"error":"state_lock_poisoned"}"#.to_string(),
        );
    };
    let provided = req
        .headers
        .get("x-sonaraim-admin-key")
        .map(|s| s.trim())
        .unwrap_or_default();
    let expected = guard
        .admin_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if expected.is_none() || expected != Some(provided) {
        return (
            401,
            "Unauthorized",
            r#"{"error":"unauthorized_admin"}"#.to_string(),
        );
    }
    let did = json_str(&req.body, "tenant_did")
        .or_else(|| json_str(&req.body, "did"))
        .unwrap_or_default();
    let api_key = json_str(&req.body, "api_key").unwrap_or_default();
    if did.trim().is_empty() || api_key.trim().is_empty() {
        return (
            400,
            "Bad Request",
            r#"{"error":"missing_tenant_did_or_api_key"}"#.to_string(),
        );
    }
    let tier = parse_tier(&json_str(&req.body, "tier").unwrap_or_default());
    let max_seats = json_u64(&req.body, "max_seats").unwrap_or(10) as u32;
    guard.register_tenant(did.trim(), api_key.trim(), tier, max_seats);
    persist_if_configured(&guard);
    (
        201,
        "Created",
        format!(
            r#"{{"status":"registered","tenant_did":"{}","tier":"{}","max_seats":{}}}"#,
            did.trim(),
            tier.as_str(),
            max_seats
        ),
    )
}

/// Executes a Redis RESP `INCR` + `EXPIRE` fixed-window check over TCP.
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
fn test_now_offset_secs() -> u64 {
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

/// Enforces promotional rate limits and `ConsentGate` token validation for a tenant dispatch.
pub(crate) fn authorize_promo(
    guard: &mut B2bState,
    tenant_did: &str,
    recipient: &str,
    consent_sig: Option<String>,
) -> Option<(u16, &'static str, String)> {
    let limit = guard.config.promo_rate_limit_rps as u64;
    let window_secs = promo_window_secs();
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_add(test_now_offset_secs());
    let window_key = format!("b2b:promo:{tenant_did}:{w}", w = now_secs / window_secs);
    let redis_ok = guard
        .redis_url
        .as_deref()
        .and_then(|u| check_redis_rate_limit_with_window(u, &window_key, limit, window_secs));
    if let Some(tenant) = guard.tenants.get_mut(tenant_did) {
        if tenant.promo_window_start != now_secs / window_secs {
            tenant.promo_window_start = now_secs / window_secs;
            tenant.promo_window_count = 0;
        }
        tenant.promo_window_count = tenant.promo_window_count.saturating_add(1);
        tenant.promo_requests = tenant.promo_requests.saturating_add(1);
        if redis_ok == Some(false) || tenant.promo_window_count > limit {
            return Some((
                429,
                "Too Many Requests",
                r#"{"error":"promo_rate_limit_exceeded"}"#.to_string(),
            ));
        }
    }
    let is_revoked = guard
        .revoked_consents
        .contains(&format!("{tenant_did}|{recipient}"));
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let token = consent_sig.map(|sig| ConsentToken {
        token_id: format!("tok_{recipient}"),
        tenant_did: tenant_did.to_string(),
        recipient_did: recipient.to_string(),
        expires_at_unix: now_unix.saturating_add(3600),
        revoked: is_revoked,
        signature: sig,
    });
    if let Err(err) = ConsentGate::authorize_dispatch(
        tenant_did,
        true,
        recipient,
        MessageCategory::Promotional,
        token.as_ref(),
        now_unix,
    ) {
        let code = match err {
            ConsentError::MissingConsentToken => "missing_consent_token",
            ConsentError::RevokedConsentToken(_) => "consent_revoked",
            ConsentError::ExpiredConsentToken { .. } => "consent_expired",
            ConsentError::InvalidConsentScope(_) => "consent_scope_invalid",
            ConsentError::UnverifiedTenantDid(_) => "unverified_tenant_did",
        };
        return Some((403, "Forbidden", format!(r#"{{"error":"{code}"}}"#)));
    }
    None
}

/// Renders Prometheus exposition format metrics (`GET /metrics`) for `sonaraim-b2b`.
pub fn handle_metrics(shared: &Arc<Mutex<B2bState>>) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return (
            500,
            "Internal Server Error",
            r#"{"error":"state_lock_poisoned"}"#.to_string(),
        );
    };
    let tenants_total = guard.tenants.len();
    let messages_total: u64 = guard.tenants.values().map(|t| t.messages_sent).sum();
    let actions_total: u64 = guard.tenants.values().map(|t| t.card_actions).sum();
    let body = format!(
        "# HELP sonaraim_b2b_tenants_total Total registered enterprise tenants\n# TYPE sonaraim_b2b_tenants_total gauge\nsonaraim_b2b_tenants_total {tenants_total}\n# HELP sonaraim_b2b_messages_sent_total Total B2B messages dispatched\n# TYPE sonaraim_b2b_messages_sent_total counter\nsonaraim_b2b_messages_sent_total {messages_total}\n# HELP sonaraim_b2b_card_actions_total Total signed card actions processed\n# TYPE sonaraim_b2b_card_actions_total counter\nsonaraim_b2b_card_actions_total {actions_total}\n"
    );
    (200, "OK", body)
}

/// Handles `POST /v1/tenants/rotate-key` to atomically rotate a tenant's Bearer API key.
pub fn handle_rotate_tenant_key(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return (
            500,
            "Internal Server Error",
            r#"{"error":"state_lock_poisoned"}"#.to_string(),
        );
    };
    let Some(tenant_did) = guard.authenticate(req) else {
        return (
            401,
            "Unauthorized",
            r#"{"error":"unauthorized"}"#.to_string(),
        );
    };
    let new_key = json_str(&req.body, "new_api_key").unwrap_or_default();
    if new_key.trim().is_empty() {
        return (
            400,
            "Bad Request",
            r#"{"error":"missing_new_api_key"}"#.to_string(),
        );
    }
    let trimmed_new = new_key.trim().to_string();
    let old_key = guard
        .tenants
        .get_mut(&tenant_did)
        .map(|tenant| std::mem::replace(&mut tenant.api_key, trimmed_new.clone()));
    if let Some(old) = old_key {
        guard.api_keys.remove(&old);
    }
    guard.api_keys.insert(trimmed_new, tenant_did.clone());
    persist_if_configured(&guard);
    (
        200,
        "OK",
        format!(r#"{{"status":"rotated","tenant_did":"{tenant_did}"}}"#),
    )
}
