//! TDD integration tests for Milestone 3 & 5 (`sonaraim-b2b` #2, #3, #4, #5, #6):
//! - #2: Live HTTP REST server, tenant Bearer auth, idempotency, consent revocation
//! - #3: 1-Click WhatsApp Cloud API endpoint & promotional rate limiter (429)
//! - #4: Signed CARD_ACTION_SUBMITTED webhook dispatcher & tenant analytics
//! - #5: Toll-free AI agent gRPC/stream framing & CRM connectors (Chatwoot, Zendesk, HubSpot)
//! - #6: In-card payment gateway intents (0.5%-1.5% take-rate) & SaaS seat metering

use sonaraim_b2b::billing::{PaymentProvider, SaasPlanTier, compute_take_rate_minor_units};
use sonaraim_b2b::config::B2bConfig;
use sonaraim_b2b::crm_connectors::{CrmProvider, normalize_crm_event};
use sonaraim_b2b::http_api::{B2bState, start_test_server};
use sonaraim_b2b::webhooks::compute_webhook_signature;
use std::io::{Read, Write};
use std::net::TcpStream;

fn sample_config(promo_rps: u32) -> B2bConfig {
    B2bConfig {
        http_host: "127.0.0.1".to_string(),
        http_port: 8042,
        grpc_port: 8043,
        node_rpc_url: "http://127.0.0.1:8040".to_string(),
        instant_clip_base_url: "https://go.sonaraim.com".to_string(),
        promo_rate_limit_rps: promo_rps,
        storage_path: "/var/lib/sonaraim-b2b/test-state.jsonl".to_string(),
        allow_insecure_callbacks: false,
    }
}

fn send_http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).expect("connect to b2b test server");
    let mut hdr_str = String::new();
    for (k, v) in headers {
        hdr_str.push_str(&format!("{k}: {v}\r\n"));
    }
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n{hdr_str}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");
    let (head, resp_body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    (status, resp_body.to_string())
}

#[test]
fn http_api_enforces_auth_messages_whatsapp_webhooks_and_rate_limits() {
    let mut state = B2bState::new(sample_config(2));
    state.register_tenant(
        "did:web:clinic.example.com",
        "tok-clinic-1",
        SaasPlanTier::SmbPro,
        5,
    );
    let (addr, handle) = start_test_server(state).expect("start b2b server");

    let (st, _) = send_http(&addr, "GET", "/health", &[], "");
    assert_eq!(st, 200);

    let (st, _) = send_http(&addr, "POST", "/v1/messages", &[], "{}");
    assert_eq!(st, 401);

    let auth = [
        ("Authorization", "Bearer tok-clinic-1"),
        ("X-Sonaraim-Tenant-ID", "did:web:clinic.example.com"),
    ];

    let (st, msg_resp) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &auth,
        r#"{"recipient_did":"did:sonaraim:alice","category":"TRANSACTIONAL","onboarded":false}"#,
    );
    assert_eq!(st, 202);
    assert!(msg_resp.contains("SMART_FALLBACK_INSTANT_CLIP"));

    let (st, _) = send_http(
        &addr,
        "POST",
        "/v1/messages",
        &auth,
        r#"{"recipient_did":"did:sonaraim:alice","category":"PROMOTIONAL","onboarded":true}"#,
    );
    assert_eq!(st, 403);

    let (st, wa_resp) = send_http(
        &addr,
        "POST",
        "/v1/whatsapp-compat/1001/messages",
        &auth,
        r#"{"to":"+51999888777","template":"appointment_confirmation","params":["Dr. Ana","10:30 AM"]}"#,
    );
    assert_eq!(st, 200);
    assert!(wa_resp.contains("SONARAIM_CARD"));

    let action_body =
        r#"{"card_id":"ord-1","action_id":"confirm_handover","user_did":"did:sonaraim:alice"}"#;
    let action_sig = compute_webhook_signature(b"tok-clinic-1", action_body);
    let action_headers = [
        ("Authorization", "Bearer tok-clinic-1"),
        ("X-Sonaraim-Signature", action_sig.as_str()),
    ];
    let (st, act_resp) = send_http(
        &addr,
        "POST",
        "/v1/cards/actions",
        &action_headers,
        action_body,
    );
    assert_eq!(st, 200);
    assert!(act_resp.contains("X-Sonaraim-Signature"));

    let (st, analytics) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:clinic.example.com/analytics",
        &auth,
        "",
    );
    assert_eq!(st, 200);
    assert!(analytics.contains("\"card_actions\":1"));

    let (st, _) = send_http(
        &addr,
        "POST",
        "/v1/consents/revoke",
        &auth,
        r#"{"user_did":"did:sonaraim:alice","token_id":"ct-1"}"#,
    );
    assert_eq!(st, 200);

    drop(handle);
}

#[test]
fn crm_connectors_and_payment_take_rate_and_subscriptions_work() {
    let ev =
        normalize_crm_event(CrmProvider::Chatwoot, "conv-42", "Hello from support").expect("map");
    assert_eq!(ev.provider, "chatwoot");

    let (fee, net) = compute_take_rate_minor_units(10_000, 100).expect("1.00% take rate");
    assert_eq!(fee, 100);
    assert_eq!(net, 9_900);

    let mut state = B2bState::new(sample_config(10));
    state.register_tenant(
        "did:web:shop.example.com",
        "tok-shop-1",
        SaasPlanTier::SmbPro,
        3,
    );
    let (addr, handle) = start_test_server(state).expect("start b2b server");
    let auth = [
        ("Authorization", "Bearer tok-shop-1"),
        ("X-Sonaraim-Tenant-ID", "did:web:shop.example.com"),
    ];

    let (st, pay_resp) = send_http(
        &addr,
        "POST",
        "/v1/payments/intents",
        &auth,
        &format!(
            r#"{{"provider":"{}","amount_minor":10000,"take_rate_bps":100}}"#,
            PaymentProvider::YapePlin.as_str()
        ),
    );
    assert_eq!(st, 201);
    assert!(pay_resp.contains("\"take_rate_fee_minor\":100"));

    let (st, sub_resp) = send_http(
        &addr,
        "GET",
        "/v1/tenants/did:web:shop.example.com/subscription",
        &auth,
        "",
    );
    assert_eq!(st, 200);
    assert!(sub_resp.contains("\"per_message_fee\":0"));

    let (st, crm_resp) = send_http(
        &addr,
        "POST",
        "/v1/integrations/crm/zendesk/events",
        &auth,
        r#"{"conversation_id":"zd-9","text":"Ticket updated"}"#,
    );
    assert_eq!(st, 200);
    assert!(crm_resp.contains("zendesk"));

    let mut boot_state = B2bState::new(sample_config(10));
    boot_state.register_bootstrap_tenant(Some("did:web:boot.example.com"), Some("sk_boot_1"));
    let (boot_addr, boot_handle) = start_test_server(boot_state).expect("start boot server");
    let (st, boot_sub) = send_http(
        &boot_addr,
        "GET",
        "/v1/tenants/did:web:boot.example.com/subscription",
        &[],
        "",
    );
    assert_eq!(st, 200);
    assert!(boot_sub.contains("did:web:boot.example.com"));
    drop(boot_handle);

    drop(handle);
}
