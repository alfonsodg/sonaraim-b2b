# API Reference — `sonaraim-b2b`

> Enterprise REST Endpoints, WhatsApp Cloud Adapter, and Public Rust Interfaces exposed by `sonaraim-b2b`.

---

## 1. Authentication & Enterprise Headers

Requests to `sonaraim-b2b` authenticate with a tenant Bearer token.
Tenant-scoped routes additionally require the `{did}` path segment to match
the authenticated tenant (`403 forbidden_tenant_mismatch` otherwise).

```http
Authorization: Bearer <B2B_API_KEY>
X-Sonaraim-Signature: sha256=<hex-hmac>
X-Sonaraim-Admin-Key: <B2B_KMS_MASTER_KEY>
```

For tenant administration endpoints (`POST /v1/tenants`), the
`X-Sonaraim-Admin-Key` header must equal `B2B_KMS_MASTER_KEY`.

> Roadmap: `X-Sonaraim-Tenant-DID` and `X-Sonaraim-Idempotency-Key` headers
> are not implemented yet.

---

## 2. Health & Prometheus Telemetry

### `GET /health`

Returns service operational status.

**Response (`200 OK`)**:

```json
{
  "status": "ok",
  "service": "sonaraim-b2b"
}
```

### `GET /metrics`

Exposes Prometheus telemetry metrics (`text/plain; version=0.0.4`).

**Response Metrics Output (`200 OK`)**:

```text
# HELP sonaraim_b2b_tenants_total Total registered enterprise tenants
# TYPE sonaraim_b2b_tenants_total gauge
sonaraim_b2b_tenants_total 1
# HELP sonaraim_b2b_messages_sent_total Total B2B messages dispatched
# TYPE sonaraim_b2b_messages_sent_total counter
sonaraim_b2b_messages_sent_total 1
# HELP sonaraim_b2b_card_actions_total Total signed card actions processed
# TYPE sonaraim_b2b_card_actions_total counter
sonaraim_b2b_card_actions_total 0
```

---

## 3. Tenant Lifecycle Endpoints

### `POST /v1/tenants`

Provisions a new enterprise tenant with a verified `did:web:` identifier,
seat quota, and API key. Requires `X-Sonaraim-Admin-Key: <B2B_KMS_MASTER_KEY>`.

**Request Payload**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "api_key": "sec_tenant_key_9941",
  "tier": "enterprise_sovereign",
  "max_seats": 25
}
```

**Response (`201 Created`)**:

```json
{
  "status": "registered",
  "tenant_did": "did:web:acme.example.com",
  "tier": "enterprise_sovereign",
  "max_seats": 25
}
```

### `POST /v1/tenants/rotate-key`

Rotates a tenant's Bearer API key; the old key is revoked immediately.
Requires the tenant's current Bearer token.

**Request Payload**:

```json
{
  "new_api_key": "sec_tenant_key_v2_9942"
}
```

**Response (`200 OK`)**:

```json
{
  "status": "rotated",
  "tenant_did": "did:web:acme.example.com"
}
```

### `GET /v1/tenants/{tenant_did}/analytics`

Requires the tenant's own Bearer token (`401` without token,
`403 forbidden_tenant_mismatch` for another tenant's DID).

**Response (`200 OK`)**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "messages_sent": 12,
  "card_actions": 3
}
```

### `GET /v1/tenants/{tenant_did}/subscription`

Requires the tenant's own Bearer token (`401` without token,
`403 forbidden_tenant_mismatch` for another tenant's DID).

**Response (`200 OK`)**:

```json
{
  "tenant_did": "did:web:acme.example.com",
  "tier": "smb_pro",
  "max_seats": 25,
  "per_message_fee": 0
}
```

---

## 4. Message Dispatch & Card Action Endpoints

### `POST /v1/messages`

Dispatches an E2EE message or declarative `Sonaraim Card` to a recipient.
Promotional messages require a valid consent token and are subject to the
fixed-window rate limiter (`B2B_PROMO_RATE_LIMIT_RPS` per
`B2B_PROMO_WINDOW_SECS`, default 1s). Requires a tenant Bearer token.

**Request Payload**:

```json
{
  "recipient_did": "did:sonaraim:alice",
  "category": "TRANSACTIONAL",
  "onboarded": false
}
```

**Response (`202 Accepted`, native route)**:

```json
{
  "status": "queued",
  "route": "NATIVE_MLS_E2EE",
  "recipient": "did:sonaraim:alice",
  "relayed": true
}
```

**Response (`202 Accepted`, fallback route)**:

```json
{
  "status": "queued",
  "route": "SMART_FALLBACK_INSTANT_CLIP",
  "magic_link": "https://clip.sonaraim.io/c/clip_abc"
}
```

### `POST /v1/cards/actions`

Inbound webhook invoked when a user executes an action inside a
`Sonaraim Card`. Requires the tenant Bearer token, a valid
`X-Sonaraim-Signature` (or `user_signature`) HMAC, and a fresh
`webhook_timestamp` inside `B2B_WEBHOOK_REPLAY_SECS` (default 300s).

**Request Payload**:

```json
{
  "card_id": "ord_0193218a",
  "action_id": "act_confirm_order",
  "tenant_did": "did:web:acme.example.com",
  "user_did": "did:sonaraim:alice",
  "webhook_timestamp": 1700000000
}
```

**Response (`200 OK`)**:

```json
{
  "status": "recorded",
  "verified": true,
  "webhook_delivered": true,
  "X-Sonaraim-Signature": "sha256=4a5b6c7d8e..."
}
```

Explicit rejection codes: `401 invalid_signature`, `400 missing_webhook_timestamp`,
`403 stale_webhook_timestamp`, `403 forbidden_tenant_mismatch`.

### `POST /v1/consents/revoke`

Revokes a user's promotional consent for the authenticated tenant.

**Request Payload**:

```json
{
  "user_did": "did:sonaraim:alice"
}
```

**Response (`200 OK`)**:

```json
{
  "status": "revoked"
}
```

### `POST /v1/payments/intents`

Creates a zero-markup payment intent for in-card transactional checkout.
Requires a tenant Bearer token.

**Request Payload**:

```json
{
  "provider": "stripe",
  "amount_minor": 10000,
  "take_rate_bps": 100
}
```

**Response (`201 Created`)**:

```json
{
  "provider": "stripe",
  "amount_minor": 10000,
  "take_rate_fee_minor": 100,
  "net_minor": 9900,
  "upstream_synced": false
}
```

---

## 5. WhatsApp Cloud API Adapter

### `POST /v1/whatsapp-compat/{phone_number_id}/messages`

Accepts standard Meta WhatsApp Business Cloud API JSON payloads and
translates them into native `Sonaraim Cards` or E2EE messages.
Requires a tenant Bearer token.

**Request Payload (Meta WhatsApp Format)**:

```json
{
  "messaging_product": "whatsapp",
  "to": "+51999888777",
  "template_name": "order_confirmation",
  "language_code": "en_US",
  "category": "UTILITY"
}
```

**Response (`200 OK`)**:

```json
{
  "messaging_product": "sonaraim",
  "format": "SONARAIM_CARD",
  "recipient": "did:sonaraim:+51999888777",
  "title": "template:order_confirmation (en_US)",
  "upstream_synced": false,
  "relayed": true
}
```

---

## 6. CRM Integration Webhooks

### `POST /v1/integrations/crm/{provider}/events`

Ingests external webhook events from CRM platforms and requires a tenant
Bearer token.

**Supported Providers**: `chatwoot`, `zendesk`, `hubspot`, `salesforce`.

**Request Payload**:

```json
{
  "conversation_id": "conv_4821",
  "text": "Agent Carlos has joined the conversation."
}
```

**Response (`200 OK`)**:

```json
{
  "status": "ingested",
  "provider": "chatwoot",
  "conversation_id": "conv_4821"
}
```

---

## 7. AI Agent Stream (TCP/JSON, toll-free)

The internal stream on `B2B_GRPC_PORT` is plain newline-delimited JSON
over TCP (`sonaraim.ai.v1` frames), not gRPC. The client sends one JSON
request line (`agent_did`, `prompt`); the server streams one `streaming`
frame per inference delta and closes with a `done` frame. Deltas come
from the pluggable HTTP backend (`B2B_AI_BACKEND_URL`, `B2B_AI_MODEL`).

---

## 8. Public Rust Crate Interfaces (`sonaraim_b2b`)

- **`config::B2bConfig`**: Environment variable loader and validator (`from_env`, `from_lookup`).
- **`consent::ConsentGate`**: Evaluates tenant authorization for transactional or promotional dispatch (`authorize_dispatch`).
- **`fallback::SmartFallbackEngine`**: Resolves delivery routes (`NativeMlsE2ee` vs `InstantClip`).
- **`whatsapp_adapter::WhatsAppCloudAdapter`**: Translates Meta templates into declarative cards.
- **`crm_connectors`**: Ingests and normalizes Chatwoot, Zendesk, HubSpot, and Salesforce events.
- **`billing::SaasPlanTier`**: Subscription tiers (`free`, `smb_pro`, `enterprise_sovereign`) and take-rate helpers.
- **`persistence`**: Hashed-credential JSONL snapshots, atomic writes, and tenant provisioning handlers.
- **`rate_limit`**: Fixed-window promotional limiter (`INCR` + `EXPIRE`) with visible Redis errors.
- **`delivery_queue::DeliveryQueue`**: Durable async delivery backlog with backoff and dead-letter file.
- **`ai_stream`**: Pluggable inference deltas for the TCP/JSON stream.
- **`webhooks`**: Replay-window verification and HMAC signature helpers.
