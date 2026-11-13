# System Diagrams — `sonaraim-b2b`

> Mermaid sequence, state, and architecture diagrams for `sonaraim-b2b`.

---

## 1. WhatsApp Migration & Message Translation Flow

```mermaid
sequenceDiagram
    autonumber
    participant CRM as Legacy CRM / ERP
    participant B2B as sonaraim-b2b (WhatsApp Adapter)
    participant Gate as ConsentGate
    participant Node as sonaraim-node

    CRM->>B2B: POST /v1/whatsapp-compat/{phone_id}/messages (Meta JSON)
    B2B->>B2B: Normalize E.164 phone & translate template to Sonaraim Card
    B2B->>Gate: authorize_dispatch(tenant_did, category, consent_token)
    Gate-->>B2B: Ok(Authorized)
    B2B->>Node: POST /v1/relay/envelopes (MLS Ciphertext)
    Node-->>B2B: 202 Accepted (queued)
    B2B-->>CRM: 200 OK (WhatsApp message_id receipt)
```

---

## 2. Smart Fallback Delivery Sequence

```mermaid
sequenceDiagram
    autonumber
    participant Biz as Enterprise Backend
    participant B2B as sonaraim-b2b
    participant Engine as SmartFallbackEngine
    participant SMS as Fallback Gateway (SMS/RCS)
    participant User as Recipient Mobile Phone

    Biz->>B2B: POST /v1/messages (recipient, card_payload)
    B2B->>Engine: resolve_route(recipient_onboarded=false)
    Engine-->>B2B: InstantClip(magic_link_url)
    B2B->>SMS: Dispatch SMS with Instant Clip URL
    SMS->>User: "Your order is arriving: https://clip.sonaraim.io/c/tok99"
    User->>User: Tap link -> Instant Web Clip opens with zero install
    User->>User: Track live order & save passkey for native app
```

---

## 3. Consent Token Verification & Revocation Flow

```mermaid
stateDiagram-v2
    [*] --> ActiveConsent: User grants promotional opt-in
    ActiveConsent --> Dispatched: Tenant sends promotional card (valid token)
    Dispatched --> ActiveConsent: Delivered & token unexpired
    ActiveConsent --> Revoked: User swipes 'Revoke Consent' (POST /v1/consents/revoke)
    Revoked --> Rejected: Tenant attempts promo dispatch (HTTP 403 Forbidden)
    Rejected --> [*]
```

---

## 4. CRM Inbound Webhook to Sonaraim Card Dispatch Flow

```mermaid
sequenceDiagram
    autonumber
    participant CRM as CRM Provider (Chatwoot/Zendesk/HubSpot)
    participant B2B as sonaraim-b2b (CRM Connector)
    participant Node as sonaraim-node
    participant App as sonaraim-app Client

    CRM->>B2B: POST /v1/integrations/crm/{provider}/events
    B2B->>B2B: Parse provider event & normalize payload
    B2B->>Node: POST /v1/relay/envelopes (Sealed Card)
    Node-->>App: Push notification -> Fetch envelope
    App->>App: Render interactive card in Services Inbox
```

---

## 5. Tenant Key Rotation Sequence

```mermaid
sequenceDiagram
    autonumber
    participant Operator as Admin Console / Operator
    participant B2B as sonaraim-b2b
    participant Store as TenantPersistence

    Operator->>B2B: POST /v1/tenants/rotate-key (new_signing_key_hex)
    B2B->>Store: Archive active key to historical keyring
    B2B->>Store: Set new key as active signing key
    B2B->>Store: Append rotation event to durable JSONL log
    B2B-->>Operator: 200 OK (key_version incremented)
```
