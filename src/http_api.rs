//! HTTP/1.1 Enterprise B2B Gateway server exposing messaging, WhatsApp compatibility,
//! Consent Gate, signed card webhooks, billing take-rate, and CRM event ingestion.

use crate::billing::SaasPlanTier;
use crate::config::B2bConfig;
use crate::consent::MessageCategory;
use crate::delivery_queue::DeliveryQueue;
use crate::fallback::{DeliveryRoute, SmartFallbackEngine};
use crate::outbound_dispatcher::{forward_to_node_relay, start_ai_stream_listener};
use crate::persistence::{
    authorize_promo, handle_metrics, handle_register_tenant, handle_rotate_tenant_key,
    persist_if_configured,
};
use crate::routes::{
    handle_card_action, handle_crm_event, handle_payment_intent, handle_revoke_consent,
    handle_tenant_analytics, handle_tenant_subscription, handle_whatsapp_compat,
};
use crate::webhooks::{
    HttpRequest, json_bool, json_str, read_http_request, write_http_response, write_json_response,
};
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
    /// Bearer API key assigned to the tenant (in-memory only, never persisted).
    pub api_key: String,
    /// SHA-256 hash of the Bearer API key used for authentication lookups.
    pub api_key_hash: String,
    /// Active SaaS subscription tier.
    pub tier: SaasPlanTier,
    /// Maximum operator seats included in the tier.
    pub max_seats: u32,
    /// Total promotional dispatch attempts tracked across all windows (observability only).
    pub promo_requests: u64,
    /// Start of the current in-memory fixed rate-limit window (`now_secs / window_secs`).
    pub promo_window_start: u64,
    /// Promotional attempts inside the current in-memory window.
    pub promo_window_count: u64,
    /// Total signed card action webhooks received.
    pub card_actions: u64,
    /// Total messages accepted for delivery.
    pub messages_sent: u64,
}

/// Tenant counters restored from a hashed at-rest snapshot (no cleartext key).
#[derive(Debug, Clone)]
pub struct RestoredTenant {
    /// Tenant decentralized identifier.
    pub did: String,
    /// SHA-256 hash of the Bearer API key.
    pub api_key_hash: String,
    /// Active SaaS subscription tier.
    pub tier: SaasPlanTier,
    /// Maximum operator seats included in the tier.
    pub max_seats: u32,
    /// Total promotional dispatch attempts across all windows.
    pub promo_requests: u64,
    /// Start of the current in-memory fixed rate-limit window.
    pub promo_window_start: u64,
    /// Promotional attempts inside the current in-memory window.
    pub promo_window_count: u64,
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
    /// Registered enterprise tenants indexed by DID.
    pub tenants: HashMap<String, TenantRecord>,
    /// Bearer API key lookup table mapping key -> tenant DID.
    pub api_keys: HashMap<String, String>,
    /// Revoked `(tenant_did|recipient_did)` consent entries.
    pub revoked_consents: HashSet<String>,
    /// Optional file path for durable JSONL state snapshots.
    pub storage_path: Option<String>,
    /// Optional admin master key for `POST /v1/tenants` provisioning.
    pub admin_key: Option<String>,
    /// Optional Redis URL for distributed promotional rate limiting.
    pub redis_url: Option<String>,
    /// Optional async delivery queue (durable jobs + dead-letter) shared by
    /// the live routes. `None` until [`crate::delivery_wiring::enable_delivery_queue`].
    /// Held behind `Arc<Mutex<..>>` so the background drain worker and the
    /// request handlers share one backlog (`Arc` keeps [`B2bState`] `Clone`).
    pub delivery_queue: Option<std::sync::Arc<std::sync::Mutex<DeliveryQueue>>>,
    /// Optional pluggable AI inference backend URL (`B2B_AI_BACKEND_URL`).
    pub ai_backend_url: Option<String>,
    /// Inference model name sent to the backend (`B2B_AI_MODEL`).
    pub ai_model: String,
}

impl B2bState {
    /// Creates a new [`B2bState`] initialized from [`B2bConfig`].
    pub fn new(config: B2bConfig) -> Self {
        Self {
            config,
            tenants: HashMap::new(),
            api_keys: HashMap::new(),
            revoked_consents: HashSet::new(),
            ai_backend_url: std::env::var("B2B_AI_BACKEND_URL")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            ai_model: std::env::var("B2B_AI_MODEL")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| "sonaraim-default".to_string()),
            storage_path: None,
            admin_key: std::env::var("B2B_KMS_MASTER_KEY").ok(),
            redis_url: std::env::var("REDIS_URL")
                .ok()
                .or_else(|| std::env::var("B2B_REDIS_URL").ok()),
            delivery_queue: None,
        }
    }

    /// Configures the admin master key required by `POST /v1/tenants`.
    pub fn set_admin_key(&mut self, key: &str) {
        self.admin_key = Some(key.to_string());
    }

    /// Configures the durable JSONL snapshot path and loads any existing state.
    pub fn set_storage_path(&mut self, path: &str) {
        self.storage_path = Some(path.to_string());
        let _ = crate::persistence::load_snapshot(self, path);
    }

    /// Configures the Redis connection URL for promotional rate limiting.
    pub fn set_redis_url(&mut self, url: &str) {
        self.redis_url = Some(url.to_string());
    }

    /// Configures the pluggable AI inference backend URL and model name.
    pub fn set_ai_backend(&mut self, url: &str, model: &str) {
        let url = url.trim();
        self.ai_backend_url = if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        };
        let model = model.trim();
        if !model.is_empty() {
            self.ai_model = model.to_string();
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
            api_key_hash: crate::persistence::hash_credential(api_key),
            tier,
            max_seats,
            promo_requests: 0,
            promo_window_start: 0,
            promo_window_count: 0,
            card_actions: 0,
            messages_sent: 0,
        };
        self.api_keys
            .insert(record.api_key_hash.clone(), did.to_string());
        self.tenants.insert(did.to_string(), record);
    }

    /// Registers a tenant restored from a hashed snapshot (no cleartext key).
    pub fn register_hashed_tenant(&mut self, restored: RestoredTenant) {
        let record = TenantRecord {
            did: restored.did.clone(),
            api_key: String::new(),
            api_key_hash: restored.api_key_hash.clone(),
            tier: restored.tier,
            max_seats: restored.max_seats,
            promo_requests: restored.promo_requests,
            promo_window_start: restored.promo_window_start,
            promo_window_count: restored.promo_window_count,
            card_actions: restored.card_actions,
            messages_sent: restored.messages_sent,
        };
        self.api_keys
            .insert(restored.api_key_hash, restored.did.clone());
        self.tenants.insert(restored.did, record);
    }

    /// Registers an optional bootstrap tenant from environment values when both are non-empty.
    pub fn register_bootstrap_tenant(&mut self, did: Option<&str>, api_key: Option<&str>) {
        if let (Some(d), Some(k)) = (did, api_key) {
            let (trimmed_did, trimmed_key) = (d.trim(), k.trim());
            if !trimmed_did.is_empty() && !trimmed_key.is_empty() {
                self.register_tenant(trimmed_did, trimmed_key, SaasPlanTier::SmbPro, 10);
            }
        }
    }

    pub(crate) fn authenticate(&self, req: &HttpRequest) -> Option<String> {
        let auth = req.headers.get("authorization")?;
        let token = auth.strip_prefix("Bearer ").unwrap_or(auth).trim();
        let digest = crate::persistence::hash_credential(token);
        self.api_keys.get(&digest).cloned()
    }
}

/// Starts the B2B HTTP server on `127.0.0.1:0` for automated integration tests.
pub fn start_test_server(state: B2bState) -> Result<(String, JoinHandle<()>), io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?.to_string();
    let shared = Arc::new(Mutex::new(state));
    crate::delivery_wiring::spawn_delivery_worker(Arc::clone(&shared));
    let handle = thread::spawn(move || serve_loop(listener, shared));
    Ok((addr, handle))
}

/// Starts the B2B HTTP server on `B2B_HTTP_HOST:B2B_HTTP_PORT` and AI stream on `B2B_GRPC_PORT`.
pub fn run_server(state: B2bState) -> Result<(), io::Error> {
    let addr = format!("{}:{}", state.config.http_host, state.config.http_port);
    let (grpc_host, grpc_port) = (state.config.http_host.clone(), state.config.grpc_port);
    let listener = TcpListener::bind(addr)?;
    let shared = Arc::new(Mutex::new(state));
    let _ = start_ai_stream_listener(&grpc_host, grpc_port, Arc::clone(&shared));
    crate::delivery_wiring::spawn_delivery_worker(Arc::clone(&shared));
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
    if req.method == "GET" && req.path == "/metrics" {
        let ct = "text/plain; version=0.0.4; charset=utf-8";
        write_http_response(&mut stream, status, reason, ct, &body);
    } else {
        write_json_response(&mut stream, status, reason, &body);
    }
}

fn route_request(req: &HttpRequest, shared: &Arc<Mutex<B2bState>>) -> (u16, &'static str, String) {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") => (
            200,
            "OK",
            r#"{"status":"ok","service":"sonaraim-b2b"}"#.to_string(),
        ),
        ("GET", "/metrics") => handle_metrics(shared),
        ("POST", "/v1/tenants") => handle_register_tenant(req, shared),
        ("POST", "/v1/tenants/rotate-key") => handle_rotate_tenant_key(req, shared),
        ("POST", "/v1/messages") => handle_dispatch_message(req, shared),
        ("POST", "/v1/cards/actions") => handle_card_action(req, shared),
        ("POST", "/v1/consents/revoke") => handle_revoke_consent(req, shared),
        ("POST", "/v1/payments/intents") => handle_payment_intent(req, shared),
        ("POST", p) if p.starts_with("/v1/whatsapp-compat/") && p.ends_with("/messages") => {
            handle_whatsapp_compat(req, shared)
        }
        ("GET", p) if p.starts_with("/v1/tenants/") && p.ends_with("/analytics") => {
            let did = p
                .trim_start_matches("/v1/tenants/")
                .trim_end_matches("/analytics");
            handle_tenant_analytics(req, did, shared)
        }
        ("GET", p) if p.starts_with("/v1/tenants/") && p.ends_with("/subscription") => {
            let did = p
                .trim_start_matches("/v1/tenants/")
                .trim_end_matches("/subscription");
            handle_tenant_subscription(req, did, shared)
        }
        ("POST", p) if p.starts_with("/v1/integrations/crm/") && p.ends_with("/events") => {
            let prov = p
                .trim_start_matches("/v1/integrations/crm/")
                .trim_end_matches("/events");
            handle_crm_event(prov, req, shared)
        }
        _ => (
            404,
            "Not Found",
            r#"{"error":"route_not_found"}"#.to_string(),
        ),
    }
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
    if category == MessageCategory::Promotional
        && let Some(err_resp) = authorize_promo(&mut guard, &tenant_did, &recipient, consent_sig)
    {
        return err_resp;
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
    let mut seq = 0u64;
    if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
        tenant.messages_sent += 1;
        seq = tenant.messages_sent;
    }
    persist_if_configured(&guard);
    let delivery_id = crate::delivery_wiring::new_delivery_id();
    drop(guard);
    match route {
        DeliveryRoute::NativeMlsE2ee {
            node_rpc_url,
            recipient_did,
        } => {
            // Durable intent first: enqueue the relay envelope (when the
            // queue is enabled) so a crash after the 202 never loses the
            // delivery. The fast path below attempts immediate dispatch and
            // drops the queued intent on success (exactly-once).
            let queued_id = if let Ok(guard) = shared.lock() {
                crate::delivery_wiring::enqueue_delivery(
                    &guard,
                    &format!("{}/v1/relay/envelopes", node_rpc_url.trim_end_matches('/')),
                    vec![],
                    &req.body,
                )
            } else {
                None
            };
            let effective_id = queued_id.as_deref().unwrap_or(&delivery_id);
            let relayed =
                forward_to_node_relay(&node_rpc_url, &tenant_did, &recipient_did, seq, &req.body);
            if relayed && let Ok(guard) = shared.lock() {
                crate::delivery_wiring::confirm_delivered(&guard, effective_id);
            }
            (
                202,
                "Accepted",
                format!(
                    r#"{{"status":"queued","delivery_id":"{effective_id}","route":"NATIVE_MLS_E2EE","recipient":"{recipient}","relayed":{relayed}}}"#
                ),
            )
        }
        DeliveryRoute::InstantClip { magic_link_url, .. } => (
            202,
            "Accepted",
            format!(
                r#"{{"status":"queued","delivery_id":"{delivery_id}","route":"SMART_FALLBACK_INSTANT_CLIP","magic_link":"{magic_link_url}"}}"#
            ),
        ),
    }
}
