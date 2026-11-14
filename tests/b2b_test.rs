//! Integration test suite for `sonaraim-b2b` covering zero-hardcode config loading,
//! transactional vs promotional `ConsentGate` enforcement, WhatsApp Business Cloud API
//! template translation, and `SmartFallbackEngine` routing.

use std::collections::HashMap;

use sonaraim_b2b::config::{B2bConfig, ConfigError};
use sonaraim_b2b::consent::{ConsentError, ConsentGate, ConsentToken, MessageCategory};
use sonaraim_b2b::fallback::{DeliveryRoute, FallbackError, SmartFallbackEngine};
use sonaraim_b2b::whatsapp_adapter::{AdapterError, WhatsAppCloudAdapter, WhatsAppTemplateRequest};

#[test]
fn config_loads_valid_environment_variables() {
    let vars = HashMap::from([
        ("B2B_HTTP_HOST", "0.0.0.0"),
        ("B2B_HTTP_PORT", "8040"),
        ("B2B_GRPC_PORT", "8041"),
        ("B2B_NODE_RPC_URL", "http://sonaraim-node:8031"),
        ("B2B_INSTANT_CLIP_BASE_URL", "https://clip.sonaraim.io/"),
        ("B2B_PROMO_RATE_LIMIT_RPS", "50"),
        ("B2B_STORAGE_PATH", "/var/lib/sonaraim-b2b/state.jsonl"),
    ]);

    let cfg = B2bConfig::from_lookup(|k| vars.get(k).map(|v| (*v).to_string()))
        .expect("expected valid B2bConfig");

    assert_eq!(cfg.http_host, "0.0.0.0");
    assert_eq!(cfg.http_port, 8040);
    assert_eq!(cfg.grpc_port, 8041);
    assert_eq!(cfg.node_rpc_url, "http://sonaraim-node:8031");
    assert_eq!(cfg.instant_clip_base_url, "https://clip.sonaraim.io");
    assert_eq!(cfg.promo_rate_limit_rps, 50);
    assert_eq!(cfg.storage_path, "/var/lib/sonaraim-b2b/state.jsonl");
    assert!(!cfg.allow_insecure_callbacks);
}

#[test]
fn config_rejects_missing_variable() {
    let err = B2bConfig::from_lookup(|_| None).expect_err("expected missing var error");
    assert_eq!(err, ConfigError::MissingVar("B2B_HTTP_HOST"));
}

#[test]
fn consent_gate_allows_transactional_for_verified_did_web_without_token() {
    let auth = ConsentGate::authorize_dispatch(
        "did:web:bank.example.com",
        true,
        "did:sonaraim:alice",
        MessageCategory::Transactional,
        None,
        1_700_000_000,
    )
    .expect("transactional message for verified did:web must succeed");

    assert_eq!(auth.tenant_did, "did:web:bank.example.com");
    assert_eq!(auth.recipient_did, "did:sonaraim:alice");
    assert_eq!(auth.category, MessageCategory::Transactional);
    assert_eq!(auth.verified_token_id, None);
}

#[test]
fn consent_gate_rejects_unverified_tenant_did() {
    let err = ConsentGate::authorize_dispatch(
        "did:web:spoofed.example.com",
        false,
        "did:sonaraim:alice",
        MessageCategory::Transactional,
        None,
        1_700_000_000,
    )
    .expect_err("unverified tenant DID must be rejected");

    assert_eq!(
        err,
        ConsentError::UnverifiedTenantDid("did:web:spoofed.example.com".to_string())
    );
}

#[test]
fn consent_gate_enforces_promotional_consent_token_lifecycle() {
    // Missing token -> rejected
    let missing_err = ConsentGate::authorize_dispatch(
        "did:web:retail.example.com",
        true,
        "did:sonaraim:alice",
        MessageCategory::Promotional,
        None,
        1_700_000_000,
    )
    .expect_err("promotional dispatch without token must fail");
    assert_eq!(missing_err, ConsentError::MissingConsentToken);

    // Valid token -> authorized
    let valid_token = ConsentToken {
        token_id: "ct-001".to_string(),
        tenant_did: "did:web:retail.example.com".to_string(),
        recipient_did: "did:sonaraim:alice".to_string(),
        expires_at_unix: 1_800_000_000,
        revoked: false,
        signature: "ed25519:deadbeef".to_string(),
    };

    let ok = ConsentGate::authorize_dispatch(
        "did:web:retail.example.com",
        true,
        "did:sonaraim:alice",
        MessageCategory::Promotional,
        Some(&valid_token),
        1_700_000_000,
    )
    .expect("valid promotional token must be authorized");
    assert_eq!(ok.verified_token_id.as_deref(), Some("ct-001"));

    // Expired token -> rejected
    let expired_err = ConsentGate::authorize_dispatch(
        "did:web:retail.example.com",
        true,
        "did:sonaraim:alice",
        MessageCategory::Promotional,
        Some(&valid_token),
        1_800_000_001,
    )
    .expect_err("expired promotional token must fail");
    assert_eq!(
        expired_err,
        ConsentError::ExpiredConsentToken {
            token_id: "ct-001".to_string(),
            expires_at_unix: 1_800_000_000,
        }
    );

    // Revoked token -> rejected
    let mut revoked_token = valid_token.clone();
    revoked_token.revoked = true;
    let revoked_err = ConsentGate::authorize_dispatch(
        "did:web:retail.example.com",
        true,
        "did:sonaraim:alice",
        MessageCategory::Promotional,
        Some(&revoked_token),
        1_700_000_000,
    )
    .expect_err("revoked promotional token must fail");
    assert_eq!(
        revoked_err,
        ConsentError::RevokedConsentToken("ct-001".to_string())
    );
}

#[test]
fn whatsapp_adapter_translates_legacy_template_into_sonaraim_card() {
    let req = WhatsAppTemplateRequest {
        messaging_product: "whatsapp".to_string(),
        to: "did:sonaraim:alice".to_string(),
        template_name: "order_confirmation".to_string(),
        language_code: "en_US".to_string(),
        category: "UTILITY".to_string(),
        body_parameters: vec!["ORD-9941".to_string(), "$42.50".to_string()],
        buttons: vec!["Track Package".to_string(), "Support".to_string()],
    };

    let card = WhatsAppCloudAdapter::translate(&req).expect("translation should succeed");
    assert_eq!(card.recipient, "did:sonaraim:alice");
    assert_eq!(card.title, "template:order_confirmation (en_US)");
    assert_eq!(card.body, "ORD-9941 | $42.50");
    assert_eq!(card.actions, vec!["Track Package", "Support"]);
    assert_eq!(card.category, MessageCategory::Transactional);

    // Marketing template maps to Promotional
    let mut promo_req = req.clone();
    promo_req.category = "MARKETING".to_string();
    let promo_card =
        WhatsAppCloudAdapter::translate(&promo_req).expect("marketing translation should succeed");
    assert_eq!(promo_card.category, MessageCategory::Promotional);

    // Non-whatsapp product rejected
    let mut bad_req = req;
    bad_req.messaging_product = "sms".to_string();
    assert_eq!(
        WhatsAppCloudAdapter::translate(&bad_req),
        Err(AdapterError::UnsupportedMessagingProduct("sms".to_string()))
    );
}

#[test]
fn smart_fallback_engine_routes_native_mls_or_instant_clip_url() {
    let engine = SmartFallbackEngine::new("http://sonaraim-node:8031", "https://clip.sonaraim.io/");

    let native_route = engine
        .resolve_route("did:sonaraim:alice", true, "unused-token")
        .expect("native route resolution should succeed");
    assert_eq!(
        native_route,
        DeliveryRoute::NativeMlsE2ee {
            node_rpc_url: "http://sonaraim-node:8031".to_string(),
            recipient_did: "did:sonaraim:alice".to_string(),
        }
    );

    let fallback_route = engine
        .resolve_route("+51999888777", false, "tok_ephemeral_99")
        .expect("fallback route resolution should succeed");
    assert_eq!(
        fallback_route,
        DeliveryRoute::InstantClip {
            magic_link_url: "https://clip.sonaraim.io/c/tok_ephemeral_99".to_string(),
            clip_token: "tok_ephemeral_99".to_string(),
        }
    );

    assert_eq!(
        engine.resolve_route("+51999888777", false, "   "),
        Err(FallbackError::EmptyClipToken)
    );
}
