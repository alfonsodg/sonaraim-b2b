# API Reference — `sonaraim-b2b`

> Enterprise REST Endpoints, WhatsApp Cloud Adapter, and Public Rust Interfaces exposed by `sonaraim-b2b`.

---

## 1. Authentication & Enterprise Headers

Requests to `sonaraim-b2b` authenticate using tenant API tokens or administrative keys:

```http
Authorization: Bearer <B2B_API_KEY>
X-Sonaraim-Tenant-DID: did:web:acme.example.com
X-Sonaraim-Idempotency-Key: 0193218a-7c40-7000-8000-1a2b3c4d5e6f
```

For tenant administration endpoints (`POST /v1/tenants`), the `B2B_ADMIN_API_KEY` or `admin_key` payload is required.

---

## 2. Health & Prometheus Telemetry

### `GET /health`

Returns service operational health, upstream `sonaraim-node` RPC status, and active tenant counts.

**Response (`200 OK`)**:

```json
{
  "status": "ok",
  "service": "sonaraim-b2b",
  "node_rpc_connected": true,
  "registered_tenants": 4,
  "promo_rate_limit_rps": 50
}
```

### `GET /metrics`

Exposes Prometheus telemetry metrics (`text/plain; version=0.0.4`).

**Response Metrics Output (`200 OK`)**:

```text
# HELP sonaraim_b2b_messages_dispatched_total Total messages dispatched
# TYPE sonaraim_b2b_messages_dispatched_total counter
sonaraim_b2b_messages_dispatched_total 128
# HELP sonaraim_b2b_rate_limit_exceeded_total Total rate limit drops
# TYPE sonaraim_b2b_rate_limit_exceeded_total counter
sonaraim_b2b_rate_limit_exceeded_total 0
```

---

## 3. Tenant Lifecycle Endpoints

### `POST /v1/tenants`

Provisions a new enterprise tenant with a verified `did:web:` identifier, seat quota, and API key.

**Request Payload**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "api_key": "sec_tenant_key_9941",
  "tier": "ENTERPRISE",
  "seats": 25,
  "admin_key": "master-admin-key-value"
}
```

**Response (`201 Created`)**:

```json
{
  "status": "provisioned",
  "tenant_did": "did:web:acme.example.com",
  "seats": 25,
  "tier": "ENTERPRISE"
}
```

### `POST /v1/tenants/rotate-key`

Rotates a tenant's active signing key, recording the old key for historical signature validation.

**Request Payload**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "new_api_key": "sec_tenant_key_v2_9942",
  "new_signing_key_hex": "4a5b6c7d8e9f..."
}
```

**Response (`200 OK`)**:

```json
{
  "status": "key_rotated",
  "tenant_did": "did:web:acme.example.com",
  "key_version": 2
}
```

### `GET /v1/tenants/{tenant_did}/analytics`

Retrieves point-in-time message delivery volume, consent token states, and conversion rates.

**Response (`200 OK`)**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "transactional_dispatched": 1204,
  "promotional_dispatched": 312,
  "consent_revocations": 4,
  "delivery_success_pct": 99.8
}
```

### `GET /v1/tenants/{tenant_did}/subscription`

Returns current seat consumption, maximum tier capacity, and flat-rate billing summary.

**Response (`200 OK`)**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "tier": "ENTERPRISE",
  "active_seats": 18,
  "max_seats": 25,
  "per_message_fee": "0.00"
}
```

---

## 4. Message Dispatch & Card Action Endpoints

### `POST /v1/messages`

Dispatches an E2EE message or declarative `Sonaraim Card` to a recipient. If the recipient has not yet onboarded and fallback is enabled, invokes the `SmartFallbackEngine`.

**Request Payload**:

```json
{
  "recipient_did": "did:sonaraim:alice",
  "recipient_phone": "+51999888777",
  "category": "TRANSACTIONAL",
  "card_title": "Order #4821 Confirmed",
  "action_label": "Track Delivery",
  "card_json": {
    "card_id": "ord_0193218a",
    "title": "Order #4821 Confirmed",
    "components": []
  }
}
```

**Response (`200 OK`)**:

```json
{
  "status": "dispatched",
  "delivery_route": "NATIVE_MLS_E2EE",
  "message_id": "msg_0193218a-7c40-7000-8000-1a2b3c4d5e6f",
  "relay_status": "forwarded"
}
```

### `POST /v1/cards/actions`

Inbound webhook invoked when a user executes an action inside a `Sonaraim Card` (form submission, payment, confirmation).

**Request Payload**:

```json
{
  "action_id": "act_confirm_order",
  "card_id": "ord_0193218a",
  "tenant_did": "did:web:acme.example.com",
  "user_did": "did:sonaraim:alice",
  "signature_hex": "4a5b6c7d8e...",
  "inputs": {
    "delivery_slot": "10:00-12:00"
  }
}
```

**Response (`200 OK`)**:

```json
{
  "status": "processed",
  "action_id": "act_confirm_order",
  "verified": true
}
```

### `POST /v1/consents/revoke`

Revokes a user's cryptographic consent token for promotional messaging from a specific tenant.

**Request Payload**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "user_did": "did:sonaraim:alice",
  "revocation_timestamp": 1700000000,
  "signature_hex": "4a5b6c7d8e..."
}
```

**Response (`200 OK`)**:

```json
{
  "status": "consent_revoked",
  "tenant_did": "did:web:acme.example.com",
  "user_did": "did:sonaraim:alice"
}
```

### `POST /v1/payments/intents`

Creates a zero-markup payment intent for in-card transactional checkout.

**Request Payload**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "amount_cents": 4250,
  "currency": "USD",
  "order_id": "ord_0193218a"
}
```

**Response (`200 OK`)**:

```json
{
  "status": "created",
  "payment_intent_id": "pi_0193218a-7c40-7000-8000",
  "amount_cents": 4250,
  "currency": "USD",
  "fee_cents": 0
}
```

---

## 5. WhatsApp Cloud API Adapter

### `POST /v1/whatsapp-compat/{phone_number_id}/messages`

Accepts standard Meta WhatsApp Business Cloud API JSON payloads and translates them transparently into native `Sonaraim Cards` or E2EE messages.

**Request Payload (Meta WhatsApp Format)**:

```json
{
  "messaging_product": "whatsapp",
  "to": "+51999888777",
  "type": "template",
  "template": {
    "name": "order_confirmation",
    "language": { "code": "en_US" },
    "components": [
      {
        "type": "body",
        "parameters": [
          { "type": "text", "text": "Alice" },
          { "type": "text", "text": "ORD-4821" }
        ]
      }
    ]
  }
}
```

**Response (`200 OK`)**:

```json
{
  "messaging_product": "whatsapp",
  "contacts": [{ "input": "+51999888777", "wa_id": "+51999888777" }],
  "messages": [{ "id": "wamid.0193218a-7c40-7000" }]
}
```

---

## 6. CRM Integration Webhooks

### `POST /v1/integrations/crm/{provider}/events`

Ingests external webhook events from CRM platforms (`chatwoot`, `zendesk`, `hubspot`, `salesforce`) and dispatches outbound conversational updates.

**Supported Providers**: `chatwoot`, `zendesk`, `hubspot`, `salesforce`.

**Request Payload**:

```json
{
  "event": "conversation_status_changed",
  "conversation_id": "conv_4821",
  "customer_phone": "+51999888777",
  "message_text": "Agent Carlos has joined the conversation."
}
```

**Response (`200 OK`)**:

```json
{
  "status": "ingested",
  "provider": "chatwoot",
  "dispatched": true
}
```

---

## 7. Public Rust Crate Interfaces (`sonaraim_b2b`)

- **`config::B2bConfig`**: Environment variable loader and validator (`from_env`, `from_lookup`).
- **`consent::ConsentGate`**: Evaluates whether a tenant has authorization to dispatch transactional or promotional messages (`authorize_dispatch`).
- **`fallback::SmartFallbackEngine`**: Resolves delivery routes (`NativeMlsE2ee` vs. `InstantClip`) based on recipient onboarding state.
- **`whatsapp_adapter::WhatsAppAdapter`**: Translates Meta templates and interactive messages into declarative `SonaraimCardDraft` objects.
- **`crm_connectors`**: Ingests and normalizes events from Chatwoot, Zendesk, HubSpot, and Salesforce.
- **`billing::TenantLedger`**: Manages flat-rate seat limits, billing tiers, and zero-markup transaction verification.
- **`persistence::TenantPersistence`**: Handles atomic JSONL durable snapshot logging.
