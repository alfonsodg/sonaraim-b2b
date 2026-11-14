//! Secondary HTTP route handlers for signed card webhooks, tenant analytics,
//! SaaS subscriptions, consent revocation, payment intents, and CRM ingestion.

use crate::billing::{PaymentProvider, compute_take_rate_minor_units};
use crate::crm_connectors::{CrmProvider, normalize_crm_event};
use crate::http_api::B2bState;
use crate::outbound_dispatcher::{
    configured_timeout_ms, forward_to_node_relay, post_json_with_retry, sync_optional_upstream,
};
use crate::url_validator::validate_callback_url;
use crate::webhooks::{
    HttpRequest, compute_webhook_signature, json_str, json_u64, verify_webhook_signature,
};
use crate::whatsapp_adapter::{WhatsAppCloudAdapter, WhatsAppTemplateRequest};
use std::sync::{Arc, Mutex};

fn err(status: u16, reason: &'static str, message: &str) -> (u16, &'static str, String) {
    (status, reason, format!(r#"{{"error":"{message}"}}"#))
}

/// Translates a legacy WhatsApp Cloud API payload into a declarative Sonaraim Card and forwards to relay.
pub(crate) fn handle_whatsapp_compat(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let (node_rpc_url, tenant_did, seq) = {
        let Ok(mut guard) = shared.lock() else {
            return err(500, "Internal Server Error", "state_lock_poisoned");
        };
        let Some(tenant_did) = guard.authenticate(req) else {
            return err(401, "Unauthorized", "unauthorized");
        };
        let mut seq = 0u64;
        if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
            tenant.messages_sent += 1;
            seq = tenant.messages_sent;
        }
        (guard.config.node_rpc_url.clone(), tenant_did, seq)
    };
    let raw_to = json_str(&req.body, "recipient_did")
        .or_else(|| json_str(&req.body, "to"))
        .unwrap_or_default();
    let recipient_did = if raw_to.is_empty() || raw_to.starts_with("did:") {
        raw_to
    } else {
        format!("did:sonaraim:{}", raw_to.trim())
    };
    let wa_req = WhatsAppTemplateRequest {
        messaging_product: json_str(&req.body, "messaging_product")
            .unwrap_or_else(|| "whatsapp".to_string()),
        to: recipient_did.clone(),
        template_name: json_str(&req.body, "template_name")
            .or_else(|| json_str(&req.body, "template"))
            .unwrap_or_else(|| "default_template".to_string()),
        language_code: json_str(&req.body, "language_code").unwrap_or_else(|| "en_US".to_string()),
        category: json_str(&req.body, "category").unwrap_or_else(|| "UTILITY".to_string()),
        body_parameters: vec![],
        buttons: vec![],
    };
    match WhatsAppCloudAdapter::translate(&wa_req) {
        Ok(card) => {
            let card_json = card.to_json();
            let relayed =
                forward_to_node_relay(&node_rpc_url, &tenant_did, &recipient_did, seq, &card_json);
            let synced = sync_optional_upstream(
                json_str(&req.body, "upstream_url"),
                "B2B_WHATSAPP_UPSTREAM_URL",
                &req.body,
            );
            (
                200,
                "OK",
                format!(
                    r#"{{"messaging_product":"sonaraim","format":"SONARAIM_CARD","recipient":"{}","title":"{}","upstream_synced":{synced},"relayed":{relayed}}}"#,
                    card.recipient, card.title
                ),
            )
        }
        Err(e) => err(400, "Bad Request", &e.to_string()),
    }
}

/// Verifies incoming card action signature, increments tenant analytics, and signs response.
pub(crate) fn handle_card_action(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    let Some(auth_tenant) = guard.authenticate(req) else {
        return err(401, "Unauthorized", "unauthorized");
    };
    let tenant_secret = guard
        .tenants
        .get(&auth_tenant)
        .map(|t| t.api_key.as_bytes().to_vec())
        .unwrap_or_default();
    let header_sig = req
        .headers
        .get("x-sonaraim-signature")
        .cloned()
        .unwrap_or_default();
    let user_sig = json_str(&req.body, "user_signature").unwrap_or(header_sig);
    if !verify_webhook_signature(&tenant_secret, &req.body, &user_sig) {
        return err(401, "Unauthorized", "invalid_signature");
    }
    if let Some(body_tenant) = json_str(&req.body, "tenant_did")
        && !body_tenant.trim().is_empty()
        && body_tenant.trim() != auth_tenant
    {
        return err(403, "Forbidden", "forbidden_tenant_mismatch");
    }
    let callback_url = json_str(&req.body, "callback_url")
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    if let Some(ref url) = callback_url
        && let Err(reason) = validate_callback_url(url, guard.config.allow_insecure_callbacks)
    {
        return err(400, "Bad Request", reason);
    }
    if let Some(tenant) = guard.tenants.get_mut(&auth_tenant) {
        tenant.card_actions += 1;
    }
    crate::persistence::persist_if_configured(&guard);
    drop(guard);
    let webhook_sig = compute_webhook_signature(&tenant_secret, &req.body);
    let delivered = callback_url.is_some_and(|url| {
        matches!(
            post_json_with_retry(
                &url,
                &[("X-Sonaraim-Signature", &webhook_sig)],
                &req.body,
                configured_timeout_ms(),
                2,
            ),
            Ok(st) if (200..300).contains(&st)
        )
    });
    (
        200,
        "OK",
        format!(
            r#"{{"status":"recorded","verified":true,"webhook_delivered":{delivered},"X-Sonaraim-Signature":"{webhook_sig}"}}"#
        ),
    )
}

/// Returns real-time message and card action analytics for a tenant DID.
pub(crate) fn handle_tenant_analytics(
    did: &str,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    match guard.tenants.get(did) {
        Some(t) => (
            200,
            "OK",
            format!(
                r#"{{"tenant_did":"{}","messages_sent":{},"card_actions":{}}}"#,
                t.did, t.messages_sent, t.card_actions
            ),
        ),
        None => err(404, "Not Found", "tenant_not_found"),
    }
}

/// Returns SaaS subscription metadata with 0% per-message markup.
pub(crate) fn handle_tenant_subscription(
    did: &str,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    match guard.tenants.get(did) {
        Some(t) => (
            200,
            "OK",
            format!(
                r#"{{"tenant_did":"{}","tier":"{}","max_seats":{},"per_message_fee":0}}"#,
                t.did,
                t.tier.as_str(),
                t.max_seats
            ),
        ),
        None => err(404, "Not Found", "tenant_not_found"),
    }
}

/// Revokes promotional consent for a `(tenant_did, user_did)` pair.
pub(crate) fn handle_revoke_consent(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    let Some(tenant_did) = guard.authenticate(req) else {
        return err(401, "Unauthorized", "unauthorized");
    };
    let user_did = json_str(&req.body, "user_did").unwrap_or_default();
    if user_did.trim().is_empty() {
        return err(400, "Bad Request", "missing_user_did");
    }
    guard
        .revoked_consents
        .insert(format!("{tenant_did}|{user_did}"));
    crate::persistence::persist_if_configured(&guard);
    (200, "OK", r#"{"status":"revoked"}"#.to_string())
}

/// Creates a payment intent and computes the platform take-rate in minor units.
pub(crate) fn handle_payment_intent(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    if guard.authenticate(req).is_none() {
        return err(401, "Unauthorized", "unauthorized");
    }
    drop(guard);
    let provider_str = json_str(&req.body, "provider").unwrap_or_default();
    let Some(provider) = PaymentProvider::parse(&provider_str) else {
        return err(400, "Bad Request", "unsupported_payment_provider");
    };
    let amount = json_u64(&req.body, "amount_minor").unwrap_or(0);
    let bps = json_u64(&req.body, "take_rate_bps").unwrap_or(50);
    match compute_take_rate_minor_units(amount, bps) {
        Ok((fee, net)) => {
            let synced = sync_optional_upstream(
                json_str(&req.body, "upstream_url"),
                "B2B_PSP_UPSTREAM_URL",
                &req.body,
            );
            (
                201,
                "Created",
                format!(
                    r#"{{"provider":"{}","amount_minor":{},"take_rate_fee_minor":{},"net_minor":{},"upstream_synced":{synced}}}"#,
                    provider.as_str(),
                    amount,
                    fee,
                    net
                ),
            )
        }
        Err(e) => err(400, "Bad Request", &e.to_string()),
    }
}

/// Normalizes and ingests an incoming CRM webhook event.
pub(crate) fn handle_crm_event(
    provider_slug: &str,
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err(500, "Internal Server Error", "state_lock_poisoned");
    };
    if guard.authenticate(req).is_none() {
        return err(401, "Unauthorized", "unauthorized");
    }
    let Some(provider) = CrmProvider::parse(provider_slug) else {
        return err(400, "Bad Request", "unsupported_crm_provider");
    };
    let conv_id = json_str(&req.body, "conversation_id").unwrap_or_default();
    let text = json_str(&req.body, "text").unwrap_or_default();
    match normalize_crm_event(provider, &conv_id, &text) {
        Ok(ev) => (
            200,
            "OK",
            format!(
                r#"{{"status":"ingested","provider":"{}","conversation_id":"{}"}}"#,
                ev.provider, ev.conversation_id
            ),
        ),
        Err(e) => err(400, "Bad Request", &e.to_string()),
    }
}
