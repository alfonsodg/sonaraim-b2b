//! HTTP/1.1 Enterprise B2B Gateway server exposing messaging, WhatsApp compatibility,
//! Consent Gate, signed card webhooks, billing take-rate, and CRM event ingestion.

use crate::billing::SaasPlanTier;
use crate::config::B2bConfig;
use crate::consent::{ConsentError, ConsentGate, ConsentToken, MessageCategory};
use crate::fallback::{DeliveryRoute, SmartFallbackEngine};
use crate::routes::{
    handle_card_action, handle_crm_event, handle_payment_intent, handle_revoke_consent,
    handle_tenant_analytics, handle_tenant_subscription, handle_whatsapp_compat,
};
use crate::webhooks::{HttpRequest, json_bool, json_str, read_http_request, write_json_response};
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Registered B2B tenant metadata and real-time analytics counters.
#[derive(Debug, Clone)]
pub struct TenantRecord {
    /// Tenant decentralized identifier (`did:sonaraim:...` or `did:web:...`).
    pub did: String,
    /// Bearer API key assigned to the tenant.
    pub api_key: String,
    /// Active SaaS subscription tier.
    pub tier: SaasPlanTier,
    /// Maximum operator seats included in the tier.
    pub max_seats: u32,
    /// Total promotional dispatch attempts tracked for rate limiting.
    pub promo_requests: u64,
    /// Total signed card action webhooks received.
    pub card_actions: u64,
    /// Total messages accepted for delivery.
    pub messages_sent: u64,
}

/// Shared mutable state for the Enterprise B2B HTTP API server.
#[derive(Debug, Clone)]
pub struct B2bState {
    /// Zero-hardcode runtime configuration.
    pub config: B2bConfig,
    pub(crate) tenants: HashMap<String, TenantRecord>,
    pub(crate) api_keys: HashMap<String, String>,
    pub(crate) revoked_consents: HashSet<String>,
}

impl B2bState {
    /// Creates a new [`B2bState`] initialized from [`B2bConfig`].
    pub fn new(config: B2bConfig) -> Self {
        Self {
            config,
            tenants: HashMap::new(),
            api_keys: HashMap::new(),
            revoked_consents: HashSet::new(),
        }
    }

    /// Registers an enterprise tenant with an API key, SaaS tier, and seat quota.
    pub fn register_tenant(
        &mut self,
        did: &str,
        api_key: &str,
        tier: SaasPlanTier,
        max_seats: u32,
    ) {
        let record = TenantRecord {
            did: did.to_string(),
            api_key: api_key.to_string(),
            tier,
            max_seats,
            promo_requests: 0,
            card_actions: 0,
            messages_sent: 0,
        };
        self.api_keys.insert(api_key.to_string(), did.to_string());
        self.tenants.insert(did.to_string(), record);
    }

    pub(crate) fn authenticate(&self, req: &HttpRequest) -> Option<String> {
        let auth = req.headers.get("authorization")?;
        let token = auth.strip_prefix("Bearer ").unwrap_or(auth).trim();
        self.api_keys.get(token).cloned()
    }
}

/// Starts the B2B HTTP server on `127.0.0.1:0` for automated integration tests.
pub fn start_test_server(state: B2bState) -> Result<(String, JoinHandle<()>), io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?.to_string();
    let shared = Arc::new(Mutex::new(state));
    let handle = thread::spawn(move || serve_loop(listener, shared));
    Ok((addr, handle))
}

/// Starts the B2B HTTP server on the configured `B2B_HTTP_HOST:B2B_HTTP_PORT`.
pub fn run_server(state: B2bState) -> Result<(), io::Error> {
    let addr = format!("{}:{}", state.config.http_host, state.config.http_port);
    let listener = TcpListener::bind(addr)?;
    let shared = Arc::new(Mutex::new(state));
    serve_loop(listener, shared);
    Ok(())
}

fn serve_loop(listener: TcpListener, shared: Arc<Mutex<B2bState>>) {
    for stream in listener.incoming().flatten() {
        let state_ref = Arc::clone(&shared);
        thread::spawn(move || handle_connection(stream, state_ref));
    }
}

fn handle_connection(mut stream: TcpStream, shared: Arc<Mutex<B2bState>>) {
    let Some(req) = read_http_request(&mut stream) else {
        return;
    };
    let (status, reason, body) = route_request(&req, &shared);
    write_json_response(&mut stream, status, reason, &body);
}

fn route_request(req: &HttpRequest, shared: &Arc<Mutex<B2bState>>) -> (u16, &'static str, String) {
    if req.method == "GET" && req.path == "/health" {
        return (
            200,
            "OK",
            r#"{"status":"ok","service":"sonaraim-b2b"}"#.to_string(),
        );
    }
    if req.method == "POST" && req.path == "/v1/messages" {
        return handle_dispatch_message(req, shared);
    }
    if req.method == "POST"
        && req.path.starts_with("/v1/whatsapp-compat/")
        && req.path.ends_with("/messages")
    {
        return handle_whatsapp_compat(req, shared);
    }
    if req.method == "POST" && req.path == "/v1/cards/actions" {
        return handle_card_action(req, shared);
    }
    if req.method == "GET"
        && req.path.starts_with("/v1/tenants/")
        && req.path.ends_with("/analytics")
    {
        let did = req
            .path
            .trim_start_matches("/v1/tenants/")
            .trim_end_matches("/analytics");
        return handle_tenant_analytics(did, shared);
    }
    if req.method == "GET"
        && req.path.starts_with("/v1/tenants/")
        && req.path.ends_with("/subscription")
    {
        let did = req
            .path
            .trim_start_matches("/v1/tenants/")
            .trim_end_matches("/subscription");
        return handle_tenant_subscription(did, shared);
    }
    if req.method == "POST" && req.path == "/v1/consents/revoke" {
        return handle_revoke_consent(req, shared);
    }
    if req.method == "POST" && req.path == "/v1/payments/intents" {
        return handle_payment_intent(req, shared);
    }
    if req.method == "POST"
        && req.path.starts_with("/v1/integrations/crm/")
        && req.path.ends_with("/events")
    {
        let provider = req
            .path
            .trim_start_matches("/v1/integrations/crm/")
            .trim_end_matches("/events");
        return handle_crm_event(provider, req, shared);
    }
    (
        404,
        "Not Found",
        r#"{"error":"route_not_found"}"#.to_string(),
    )
}

fn handle_dispatch_message(
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
    let recipient = json_str(&req.body, "recipient_did")
        .or_else(|| json_str(&req.body, "to"))
        .unwrap_or_default();
    let category_str =
        json_str(&req.body, "category").unwrap_or_else(|| "TRANSACTIONAL".to_string());
    let onboarded = json_bool(&req.body, "onboarded")
        .or_else(|| json_bool(&req.body, "recipient_onboarded"))
        .unwrap_or(false);
    let consent_sig = json_str(&req.body, "consent_token");

    let category = match category_str.to_ascii_uppercase().as_str() {
        "PROMOTIONAL" | "MARKETING" => MessageCategory::Promotional,
        _ => MessageCategory::Transactional,
    };
    if category == MessageCategory::Promotional {
        let limit = guard.config.promo_rate_limit_rps as u64;
        if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
            tenant.promo_requests += 1;
            if tenant.promo_requests > limit {
                return (
                    429,
                    "Too Many Requests",
                    r#"{"error":"promo_rate_limit_exceeded"}"#.to_string(),
                );
            }
        }
        let consent_key = format!("{tenant_did}|{recipient}");
        let is_revoked = guard.revoked_consents.contains(&consent_key);
        let now_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let token = consent_sig.map(|sig| ConsentToken {
            token_id: format!("tok_{recipient}"),
            tenant_did: tenant_did.clone(),
            recipient_did: recipient.clone(),
            expires_at_unix: now_unix.saturating_add(3600),
            revoked: is_revoked,
            signature: sig,
        });
        if let Err(err) = ConsentGate::authorize_dispatch(
            &tenant_did,
            true,
            &recipient,
            category,
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
            return (403, "Forbidden", format!(r#"{{"error":"{code}"}}"#));
        }
    }

    let engine = SmartFallbackEngine::new(
        &guard.config.node_rpc_url,
        &guard.config.instant_clip_base_url,
    );
    let clip_token = format!("clip_{}", recipient.replace(':', "_"));
    let Ok(route) = engine.resolve_route(&recipient, onboarded, &clip_token) else {
        return (
            400,
            "Bad Request",
            r#"{"error":"invalid_recipient"}"#.to_string(),
        );
    };
    if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
        tenant.messages_sent += 1;
    }
    match route {
        DeliveryRoute::NativeMlsE2ee { .. } => (
            202,
            "Accepted",
            format!(r#"{{"status":"queued","route":"NATIVE_MLS_E2EE","recipient":"{recipient}"}}"#),
        ),
        DeliveryRoute::InstantClip { magic_link_url, .. } => (
            202,
            "Accepted",
            format!(
                r#"{{"status":"queued","route":"SMART_FALLBACK_INSTANT_CLIP","magic_link":"{magic_link_url}"}}"#
            ),
        ),
    }
}
