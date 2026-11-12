//! Secondary HTTP route handlers for signed card webhooks, tenant analytics,
//! SaaS subscriptions, consent revocation, payment intents, and CRM ingestion.

use crate::billing::{PaymentProvider, compute_take_rate_minor_units};
use crate::crm_connectors::{CrmProvider, normalize_crm_event};
use crate::http_api::B2bState;
use crate::outbound_dispatcher::{
    configured_timeout_ms, post_json_with_retry, sync_optional_upstream,
};
use crate::webhooks::{
    HttpRequest, compute_webhook_signature, json_str, json_u64, verify_webhook_signature,
};
use crate::whatsapp_adapter::{WhatsAppCloudAdapter, WhatsAppTemplateRequest};
use std::sync::{Arc, Mutex};

fn err_500() -> (u16, &'static str, String) {
    (
        500,
        "Internal Server Error",
        r#"{"error":"state_lock_poisoned"}"#.to_string(),
    )
}

fn err_401() -> (u16, &'static str, String) {
    (
        401,
        "Unauthorized",
        r#"{"error":"unauthorized"}"#.to_string(),
    )
}

/// Translates a legacy WhatsApp Cloud API payload into a declarative Sonaraim Card.
pub(crate) fn handle_whatsapp_compat(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err_500();
    };
    if guard.authenticate(req).is_none() {
        return err_401();
    }
    drop(guard);
    let wa_req = WhatsAppTemplateRequest {
        messaging_product: json_str(&req.body, "messaging_product")
            .unwrap_or_else(|| "whatsapp".to_string()),
        to: json_str(&req.body, "to").unwrap_or_default(),
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
            let synced = sync_optional_upstream(
                json_str(&req.body, "upstream_url"),
                "B2B_WHATSAPP_UPSTREAM_URL",
                &req.body,
            );
            (
                200,
                "OK",
                format!(
                    r#"{{"messaging_product":"sonaraim","format":"SONARAIM_CARD","recipient":"{}","title":"{}","upstream_synced":{synced}}}"#,
                    card.recipient, card.title
                ),
            )
        }
        Err(err) => (400, "Bad Request", format!(r#"{{"error":"{err}"}}"#)),
    }
}

/// Verifies incoming card action signature, increments tenant analytics, and signs response.
pub(crate) fn handle_card_action(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return err_500();
    };
    let Some(auth_tenant) = guard.authenticate(req) else {
        return err_401();
    };
    let header_sig = req
        .headers
        .get("x-sonaraim-signature")
        .cloned()
        .unwrap_or_default();
    let user_sig = json_str(&req.body, "user_signature").unwrap_or(header_sig);
    if !verify_webhook_signature(&req.body, &user_sig) {
        return (
            401,
            "Unauthorized",
            r#"{"error":"invalid_signature"}"#.to_string(),
        );
    }
    let tenant_did = json_str(&req.body, "tenant_did").unwrap_or(auth_tenant);
    if let Some(tenant) = guard.tenants.get_mut(&tenant_did) {
        tenant.card_actions += 1;
    }
    crate::persistence::persist_if_configured(&guard);
    drop(guard);
    let webhook_sig = compute_webhook_signature(&req.body);
    let delivered = json_str(&req.body, "callback_url")
        .filter(|u| !u.trim().is_empty())
        .is_some_and(|url| {
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
        return err_500();
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
        None => (
            404,
            "Not Found",
            r#"{"error":"tenant_not_found"}"#.to_string(),
        ),
    }
}

/// Returns SaaS subscription metadata with 0% per-message markup.
pub(crate) fn handle_tenant_subscription(
    did: &str,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err_500();
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
        None => (
            404,
            "Not Found",
            r#"{"error":"tenant_not_found"}"#.to_string(),
        ),
    }
}

/// Revokes promotional consent for a `(tenant_did, user_did)` pair.
pub(crate) fn handle_revoke_consent(
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(mut guard) = shared.lock() else {
        return err_500();
    };
    let Some(tenant_did) = guard.authenticate(req) else {
        return err_401();
    };
    let user_did = json_str(&req.body, "user_did").unwrap_or_default();
    if user_did.trim().is_empty() {
        return (
            400,
            "Bad Request",
            r#"{"error":"missing_user_did"}"#.to_string(),
        );
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
        return err_500();
    };
    if guard.authenticate(req).is_none() {
        return err_401();
    }
    drop(guard);
    let provider_str = json_str(&req.body, "provider").unwrap_or_default();
    let Some(provider) = PaymentProvider::parse(&provider_str) else {
        return (
            400,
            "Bad Request",
            r#"{"error":"unsupported_payment_provider"}"#.to_string(),
        );
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
        Err(err) => (400, "Bad Request", format!(r#"{{"error":"{err}"}}"#)),
    }
}

/// Normalizes and ingests an incoming CRM webhook event.
pub(crate) fn handle_crm_event(
    provider_slug: &str,
    req: &HttpRequest,
    shared: &Arc<Mutex<B2bState>>,
) -> (u16, &'static str, String) {
    let Ok(guard) = shared.lock() else {
        return err_500();
    };
    if guard.authenticate(req).is_none() {
        return err_401();
    }
    let Some(provider) = CrmProvider::parse(provider_slug) else {
        return (
            400,
            "Bad Request",
            r#"{"error":"unsupported_crm_provider"}"#.to_string(),
        );
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
        Err(err) => (400, "Bad Request", format!(r#"{{"error":"{err}"}}"#)),
    }
}
