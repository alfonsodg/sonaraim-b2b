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
            r#"{{"kind":"tenant","did":"{}","api_key":"{}","tier":"{}","max_seats":{},"promo_requests":{},"card_actions":{},"messages_sent":{}}}"#,
            t.did, t.api_key, t.tier.as_str(), t.max_seats, t.promo_requests, t.card_actions, t.messages_sent
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

/// Executes a Redis RESP `INCR` command over TCP and returns `Some(count <= limit)`.
pub fn check_redis_rate_limit(redis_url: &str, key: &str, limit: u64) -> Option<bool> {
    let stripped = redis_url
        .trim()
        .strip_prefix("redis://")
        .unwrap_or(redis_url.trim());
    let host_port = stripped.split('/').next()?;
    let addr: SocketAddr = host_port.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(150)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(150)));
    let cmd = format!("*2\r\n$4\r\nINCR\r\n${}\r\n{key}\r\n", key.len());
    stream.write_all(cmd.as_bytes()).ok()?;
    stream.flush().ok()?;
    let mut buf = [0u8; 128];
    let n = stream.read(&mut buf).ok()?;
    let reply = String::from_utf8_lossy(&buf[..n]);
    let digits: String = reply
        .trim_start_matches(':')
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<u64>().ok().map(|c| c <= limit)
}

/// Enforces promotional rate limits and `ConsentGate` token validation for a tenant dispatch.
pub(crate) fn authorize_promo(
    guard: &mut B2bState,
    tenant_did: &str,
    recipient: &str,
    consent_sig: Option<String>,
) -> Option<(u16, &'static str, String)> {
    let limit = guard.config.promo_rate_limit_rps as u64;
    let redis_ok = guard
        .redis_url
        .as_deref()
        .and_then(|u| check_redis_rate_limit(u, &format!("b2b:promo:{tenant_did}"), limit));
    if let Some(tenant) = guard.tenants.get_mut(tenant_did) {
        tenant.promo_requests += 1;
        if redis_ok == Some(false) || tenant.promo_requests > limit {
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
