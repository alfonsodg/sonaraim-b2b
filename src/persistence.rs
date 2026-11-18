//! Durable JSONL snapshot persistence for tenants and revoked consents and
//! dynamic `POST /v1/tenants` admin provisioning handler.

use crate::billing::SaasPlanTier;
use crate::consent::{ConsentError, ConsentGate, ConsentToken, MessageCategory};
use crate::http_api::B2bState;
use crate::rate_limit::{
    check_redis_rate_limit_with_window, promo_window_secs, test_now_offset_secs,
};
use crate::webhooks::{HttpRequest, json_str, json_u64};
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Parses a [`SaasPlanTier`] identifier (case-insensitive), defaulting to [`SaasPlanTier::SmbPro`].
pub fn parse_tier(raw: &str) -> SaasPlanTier {
    match raw.trim().to_ascii_lowercase().as_str() {
        "free" => SaasPlanTier::Free,
        "enterprise_sovereign" | "enterprise" => SaasPlanTier::EnterpriseSovereign,
        _ => SaasPlanTier::SmbPro,
    }
}

/// Computes the SHA-256 hex digest used as a lookup key for a Bearer credential.
pub fn hash_credential(credential: &str) -> String {
    crate::webhooks::encode_hex(&crate::hmac_sha256::sha256(credential.as_bytes()))
}

/// Derives a deterministic per-tenant HMAC signing secret from the tenant DID
/// and the process `B2B_KMS_MASTER_KEY` (with a dev-only fallback), so the
/// webhook signing key is cryptographically separated from the Bearer API key.
pub fn derive_signing_secret(tenant_did: &str) -> Vec<u8> {
    let master = std::env::var("B2B_KMS_MASTER_KEY").unwrap_or_default();
    let master = master.trim();
    let salt = if master.is_empty() {
        "sonaraim-b2b-dev-kms-fallback-v1"
    } else {
        master
    };
    crate::hmac_sha256::hmac_sha256(
        salt.as_bytes(),
        format!("b2b-signing-v1:{tenant_did}").as_bytes(),
    )
    .to_vec()
}

/// Returns the hex-encoded signing secret for snapshot integrity checks.
fn signing_secret_hex(tenant_did: &str) -> String {
    crate::webhooks::encode_hex(&derive_signing_secret(tenant_did))
}

/// Saves all registered tenants and revoked consent keys in [`B2bState`] to a JSONL file.
///
/// Credentials at rest are SHA-256 hashes (`api_key_sha256`) plus a distinct
/// signing-secret hash (`signing_secret_sha256`); the cleartext Bearer key
/// never reaches disk. Writes are atomic: content lands in `<path>.tmp`
/// (with fsync) and is renamed over `<path>`, followed by a SHA-256
/// integrity line (`kind: checksum`) verified on load.
pub fn save_snapshot(state: &B2bState, path: &str) -> io::Result<()> {
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let mut out = String::new();
    for t in state.tenants.values() {
        out.push_str(&format!(
            r#"{{"kind":"tenant","did":"{}","api_key_sha256":"{}","signing_secret_sha256":"{}","tier":"{}","max_seats":{},"promo_requests":{},"promo_window_start":{},"promo_window_count":{},"card_actions":{},"messages_sent":{}}}"#,
            t.did,
            hash_credential(&t.api_key),
            signing_secret_hex(&t.did),
            t.tier.as_str(),
            t.max_seats,
            t.promo_requests,
            t.promo_window_start,
            t.promo_window_count,
            t.card_actions,
            t.messages_sent
        ));
        out.push('\n');
    }
    for key in &state.revoked_consents {
        out.push_str(&format!(r#"{{"kind":"revoked","key":"{key}"}}"#));
        out.push('\n');
    }
    let digest = crate::webhooks::encode_hex(&crate::hmac_sha256::sha256(out.as_bytes()));
    out.push_str(&format!(r#"{{"kind":"checksum","sha256":"{digest}"}}"#));
    out.push('\n');
    let tmp_path = format!("{path}.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(out.as_bytes())?;
        tmp.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    if let Some(parent) = Path::new(path).parent()
        && !parent.as_os_str().is_empty()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Loads registered tenants and revoked consents from a JSONL snapshot file into [`B2bState`].
///
/// The trailing `checksum` line must match the SHA-256 of all preceding
/// content; any mismatch (e.g. a kill-during-write truncation) fails closed
/// with an `InvalidData` error and leaves `state` untouched. Stored
/// `api_key_sha256` hashes are registered directly into the hashed lookup
/// table so Bearer auth keeps working without the cleartext key.
pub fn load_snapshot(state: &mut B2bState, path: &str) -> io::Result<()> {
    if !Path::new(path).exists() {
        return Ok(());
    }
    let content = fs::read_to_string(path)?;
    load_snapshot_str(state, &content)
}

/// Parses and validates snapshot content, failing closed on checksum mismatch.
fn load_snapshot_str(state: &mut B2bState, content: &str) -> io::Result<()> {
    let mut lines: Vec<&str> = content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let Some(checksum_line) = lines.pop() else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty snapshot"));
    };
    if json_str(checksum_line, "kind").as_deref() != Some("checksum") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot missing checksum line",
        ));
    }
    let expected = json_str(checksum_line, "sha256").unwrap_or_default();
    let body = if lines.is_empty() {
        String::new()
    } else {
        lines.join("\n") + "\n"
    };
    let actual = crate::webhooks::encode_hex(&crate::hmac_sha256::sha256(body.as_bytes()));
    if !crate::hmac_sha256::constant_time_eq(actual.as_bytes(), expected.as_bytes()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "snapshot checksum mismatch",
        ));
    }
    let mut tenants: Vec<crate::http_api::RestoredTenant> = Vec::new();
    let mut revoked: Vec<String> = Vec::new();
    for line in &lines {
        match json_str(line, "kind").as_deref() {
            Some("tenant") => {
                let did = json_str(line, "did").unwrap_or_default();
                let key_hash = json_str(line, "api_key_sha256")
                    .or_else(|| json_str(line, "api_key"))
                    .unwrap_or_default();
                let tier = parse_tier(&json_str(line, "tier").unwrap_or_default());
                let max_seats = json_u64(line, "max_seats").unwrap_or(10) as u32;
                if did.trim().is_empty() || key_hash.trim().is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "snapshot tenant entry invalid",
                    ));
                }
                if signing_secret_hex(&did)
                    != json_str(line, "signing_secret_sha256").unwrap_or_default()
                    && json_str(line, "signing_secret_sha256").is_some()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "snapshot signing secret mismatch",
                    ));
                }
                tenants.push(crate::http_api::RestoredTenant {
                    did,
                    api_key_hash: key_hash,
                    tier,
                    max_seats,
                    promo_requests: json_u64(line, "promo_requests").unwrap_or(0),
                    promo_window_start: json_u64(line, "promo_window_start").unwrap_or(0),
                    promo_window_count: json_u64(line, "promo_window_count").unwrap_or(0),
                    card_actions: json_u64(line, "card_actions").unwrap_or(0),
                    messages_sent: json_u64(line, "messages_sent").unwrap_or(0),
                });
            }
            Some("revoked") => {
                if let Some(k) = json_str(line, "key")
                    && !k.is_empty()
                {
                    revoked.push(k);
                }
            }
            _ => {}
        }
    }
    for tenant in tenants {
        state.register_hashed_tenant(tenant);
    }
    for k in revoked {
        state.revoked_consents.insert(k);
    }
    Ok(())
}

/// Persists [`B2bState`] to its configured `storage_path` if one is set.
pub use crate::rate_limit::{check_redis_rate_limit, last_redis_rate_limit_error};

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
    // snake_case is primary (docs/API.md); camelCase aliases keep
    // sonaraim-admin provisionB2bTenant (tenantDid/apiKey/seats) working.
    let did = json_str(&req.body, "tenant_did")
        .or_else(|| json_str(&req.body, "did"))
        .or_else(|| json_str(&req.body, "tenantDid"))
        .unwrap_or_default();
    let api_key = json_str(&req.body, "api_key")
        .or_else(|| json_str(&req.body, "apiKey"))
        .unwrap_or_default();
    if did.trim().is_empty() || api_key.trim().is_empty() {
        return (
            400,
            "Bad Request",
            r#"{"error":"missing_tenant_did_or_api_key"}"#.to_string(),
        );
    }
    let tier = parse_tier(&json_str(&req.body, "tier").unwrap_or_default());
    let max_seats = json_u64(&req.body, "max_seats")
        .or_else(|| json_u64(&req.body, "maxSeats"))
        .or_else(|| json_u64(&req.body, "seats"))
        .unwrap_or(10) as u32;
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
    let new_hash = hash_credential(&trimmed_new);
    let old_hash = guard
        .tenants
        .get_mut(&tenant_did)
        .map(|tenant| std::mem::replace(&mut tenant.api_key_hash, new_hash.clone()));
    if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
        tenant.api_key = trimmed_new;
    }
    if let Some(old) = old_hash {
        guard.api_keys.remove(&old);
    }
    guard.api_keys.insert(new_hash, tenant_did.clone());
    persist_if_configured(&guard);
    (
        200,
        "OK",
        format!(r#"{{"status":"rotated","tenant_did":"{tenant_did}"}}"#),
    )
}
