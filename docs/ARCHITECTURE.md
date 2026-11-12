# System Architecture — `sonaraim-b2b`

> Architectural overview of the Sonaraim Enterprise E2EE Bridge, Consent Gate, WhatsApp Cloud API Adapter, and Smart Fallback Engine.

## 1. Overview

`sonaraim-b2b` provides an enterprise-grade gateway that bridges existing business messaging systems into the Sonaraim end-to-end encrypted network without compromising user privacy or requiring multi-month backend rewrites.

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
|  |   (whatsapp_adapter.rs)   |     |         Endpoint          |  |
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

## 3. Component Responsibilities

### 3.1 Configuration Loader (`src/config.rs`)

Reads all runtime settings from environment variables (`B2B_HTTP_HOST`, `B2B_HTTP_PORT`, `B2B_GRPC_PORT`, `B2B_NODE_RPC_URL`, `B2B_INSTANT_CLIP_BASE_URL`, `B2B_PROMO_RATE_LIMIT_RPS`). Rejects missing or malformed values at startup.

### 3.2 Cryptographic Consent Gate (`src/consent.rs`)

Prevents spam at the protocol boundary:

- Verifies that the sender possesses a valid, verified `did:web:` identifier.
- Permits `MessageCategory::Transactional` messages for verified tenants without requiring a promotional opt-in token.
- Requires a non-expired, unrevoked `ConsentToken` matching both `tenant_did` and `recipient_did` for `MessageCategory::Promotional` messages.

### 3.3 WhatsApp Business Cloud API Adapter (`src/whatsapp_adapter.rs`)

Accepts legacy `WhatsAppTemplateRequest` structures (mirroring Meta Graph API template payloads with body parameters and quick-reply/call-to-action buttons) and transforms them into deterministic `SonaraimCardDraft` envelopes.

### 3.4 Smart Fallback Engine (`src/fallback.rs`)

Evaluates recipient onboarding state:

- When the recipient is registered on Sonaraim, dispatches the encrypted envelope directly via `sonaraim-node` MLS delivery.
- When the recipient is not yet registered, generates a zero-install Instant Web / App Clip link (`{B2B_INSTANT_CLIP_BASE_URL}/c/{ephemeral_token}`) so the recipient can complete the interactive card flow immediately in an isolated browser clip.
