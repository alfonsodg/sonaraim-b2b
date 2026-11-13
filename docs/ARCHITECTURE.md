# System Architecture — `sonaraim-b2b`

> Architectural overview of the Sonaraim Enterprise E2EE Bridge, Consent Gate, WhatsApp Cloud API Adapter, and Smart Fallback Engine.

## 1. Overview

`sonaraim-b2b` provides an enterprise-grade gateway bridging legacy business messaging systems into the Sonaraim end-to-end encrypted network without compromising user privacy, exposing plaintext metadata to relays, or charging arbitrary per-message fees.

---

## 2. High-Level Data Flow (ASCII)

```text
+-------------------------------------------------------------------+
|                     Enterprise Backend System                     |
|   (ERP / CRM / Core Banking / Legacy WhatsApp Cloud Webhook)      |
+---------------------------------+---------------------------------+
                                  |
                                  | HTTPS / gRPC (did:web Auth)
                                  v
+-------------------------------------------------------------------+
|                           sonaraim-b2b                            |
|                                                                   |
|  +---------------------------+     +---------------------------+  |
|  |  1-Click WhatsApp Adapter |     |    Native B2B Dispatch    |  |
|  |   (whatsapp_adapter.rs)   |     |       (routes.rs)         |  |
|  +-------------+-------------+     +-------------+-------------+  |
|                |                                 |                |
|                +----------------+----------------+                |
|                                 |                                 |
|                                 v                                 |
|                +---------------------------------+                |
|                |     SonaraimCardDraft Model     |                |
|                +----------------+----------------+                |
|                                 |                                 |
|                                 v                                 |
|                +---------------------------------+                |
|                |     Cryptographic ConsentGate   |                |
|                |          (consent.rs)           |                |
|                |  - Verified did:web required    |                |
|                |  - Transactional: unlimited     |                |
|                |  - Promotional: requires valid  |                |
|                |    unrevoked ConsentToken       |                |
|                +----------------+----------------+                |
|                                 |                                 |
|                                 v                                 |
|                +---------------------------------+                |
|                |       SmartFallbackEngine       |                |
|                |          (fallback.rs)          |                |
|                +-------+-----------------+-------+                |
+------------------------|-----------------|------------------------+
                         |                 |
       Recipient Onboarded                 Recipient Not Onboarded
         (MLS KeyPackage)                    (Zero-Install Clip)
                         |                 |
                         v                 v
        +--------------------+         +----------------------------+
        |   sonaraim-node    |         |   Instant Web / App Clip   |
        | (Native MLS E2EE)  |         | ({BASE_URL}/c/{clip_token})|
        +--------------------+         +----------------------------+
```

---

## 3. Subsystem Architecture

### 3.1 1-Click WhatsApp Cloud API Adapter (`src/whatsapp_adapter.rs`)

Permits businesses to migrate from Meta's WhatsApp Cloud API with zero backend code rewrites:

- **Template Parsing**: Parses WhatsApp Cloud API template requests (`name`, `language`, `components`, `parameters`).
- **Card Translation**: Translates templates into native `SonaraimCardDraft` objects with corresponding actions and zero executable code.
- **E.164 Normalization**: Normalizes telephone numbers into standard international format for routing.

### 3.2 Cryptographic Consent Gate (`src/consent.rs`)

Enforces user sovereignty and anti-spam controls at the enterprise boundary:

- **Category Separation**: Messages are categorized into `Transactional` (OTP, delivery alerts, invoices) and `Promotional` (marketing, offers).
- **Consent Tokens**: Promotional messages require an active, unexpired `ConsentToken` cryptographically signed by the user's device.
- **One-Tap Revocation**: When a user revokes consent, subsequent promotional attempts are rejected immediately with HTTP 403.

### 3.3 Smart Fallback Engine (`src/fallback.rs`)

Solves the cold-start adoption barrier for enterprise communications:

- **Onboarded Recipients**: Routes messages via native MLS 1.0 E2EE directly to `sonaraim-node`.
- **Unregistered Recipients**: Generates a secure, signed Instant Web / App Clip link (`{B2B_INSTANT_CLIP_BASE_URL}/c/{clip_token}`). Tapping the link opens a native-speed web clip where the user interacts with the card and onboards with Passkeys.

### 3.4 CRM Integration Connectors (`src/crm_connectors.rs`)

Bridges multi-seat customer support platforms into E2EE channels:

- **Supported Platforms**: Chatwoot, Zendesk, HubSpot, and Salesforce.
- **Bi-Directional Sync**: Ingests agent replies from CRMs, formats them into signed cards, and dispatches customer actions back into CRM tickets.

### 3.5 Outbound Webhook Dispatcher (`src/outbound_dispatcher.rs`, `src/webhooks.rs`)

- **Signed Callbacks**: Outbound webhooks include timestamped Ed25519/HMAC signatures in `X-Sonaraim-Signature`.
- **Retry with Exponential Backoff**: Transient network failures trigger structured retries with jittered backoff.

### 3.6 Tenant Persistence & KMS Envelope Encryption (`src/persistence.rs`)

- **Durable Snapshots**: Appends tenant state and message delivery receipts to a local JSONL log for crash recovery.
- **KMS Master Key**: Master tenant credentials are sealed using `B2B_KMS_MASTER_KEY` envelope encryption.
