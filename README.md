# Sonaraim B2B Gateway (`sonaraim-b2b`)

> Enterprise E2EE Bridge, 1-Click WhatsApp Cloud API Adapter, Cryptographic Consent Gate, and Smart Fallback Engine.

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)
[![Crate: Rust 2021](https://img.shields.io/badge/Crate-Rust%202021-DEA584.svg?logo=rust&logoColor=white)](Cargo.toml)
[![Compatibility: WhatsApp Cloud API](https://img.shields.io/badge/Compatibility-WhatsApp%20Cloud%20API-25D366.svg?logo=whatsapp&logoColor=white)](src/whatsapp_adapter.rs)
[![Governance: Cryptographic Consent Gate](https://img.shields.io/badge/Governance-Cryptographic%20Consent%20Gate-success.svg)](src/consent.rs)
[![Fallback: Smart Instant Clip](https://img.shields.io/badge/Fallback-Smart%20Instant%20Clip-blue.svg)](src/fallback.rs)

`sonaraim-b2b` connects modern enterprise business backends, CRMs, and AI agents directly to the decentralized Sonaraim messaging network without compromising user privacy, requiring complex client rewrites, or paying arbitrary per-message toll fees.

---

## ⚡ Key Capabilities

- **Enterprise E2EE Termination Bridge (`src/http_api.rs`, `src/routes.rs`)**: Terminates organization MLS 1.0 sessions and bridges encrypted communications into clean REST/gRPC events for internal CRMs and bots.
- **1-Click WhatsApp Cloud API Adapter (`src/whatsapp_adapter.rs`)**: Drop-in compatible endpoint accepting standard Meta WhatsApp Business Cloud API JSON payloads and translating them into native declarative cards.
- **Cryptographic Consent Gate (`src/consent.rs`)**: Categorizes messages into Transactional (unlimited flat-rate) and Promotional (requiring unexpired user-signed `ConsentToken` opt-ins with instant revocation).
- **Smart Fallback Engine (`src/fallback.rs`)**: Automatically routes non-onboarded recipients to Instant Web / App Clip magic links (`{B2B_INSTANT_CLIP_BASE_URL}/c/{token}`) for 100% day-one reachability.
- **Dynamic Tenant Provisioning & Key Rotation (`src/persistence.rs`, `src/routes.rs`)**: Multi-tenant onboarding, KMS master key protection (`B2B_KMS_MASTER_KEY`), and zero-downtime key rotation.
- **CRM Integration Connectors (`src/crm_connectors.rs`)**: Standardized inbound and outbound event webhooks for Chatwoot, Zendesk, HubSpot, and Salesforce.
- **Zero-Markup Payment Intents (`src/billing.rs`, `src/routes.rs`)**: In-card transactional checkout intent generation with zero per-message fees.
- **Prometheus Telemetry (`src/http_api.rs`)**: Real-time metrics counters for messages dispatched, rate limits, and active tenants.

---

## 🏗️ B2B Gateway Architecture & Message Flow

The following ASCII diagram illustrates the inbound enterprise bridging, WhatsApp API translation, consent verification, and fallback delivery mechanisms:

```text
+-----------------------------------------------------------------------------------+
|                        SONARAIM B2B GATEWAY ARCHITECTURE                          |
+-----------------------------------------------------------------------------------+
|                                                                                   |
|  [Enterprise CRMs & Bots]                   [Meta WhatsApp Cloud API Callers]     |
|  - Chatwoot, Zendesk, Salesforce            - Legacy Cloud API POST JSON payloads |
|  - AI Agent Webhooks & REST API             - Template messages & media alerts    |
|            |                                              |                       |
|            | HTTPS / REST                                 | Cloud API compat      |
|            v                                              v                       |
|  +-----------------------------------------------------------------------------+  |
|  |                     INBOUND DISPATCHER & ADAPTER LAYER                      |  |
|  |  * Meta WhatsApp Cloud API Translation Engine (whatsapp_adapter.rs)         |  |
|  |  * REST Message Dispatch Router (http_api.rs, routes.rs)                    |  |
|  +-------------------------------------+---------------------------------------+  |
|                                        |                                          |
|                                        v                                          |
|  +-----------------------------------------------------------------------------+  |
|  |                   CRYPTOGRAPHIC CONSENT GATE (consent.rs)                   |  |
|  |  - Transactional / OTP / Utility  --> UNLIMITED FLAT-RATE DISPATCH          |  |
|  |  - Promotional / Marketing Alerts --> VERIFY VALID USER ConsentToken        |  |
|  +-------------------------------------+---------------------------------------+  |
|                                        |                                          |
|               +------------------------+------------------------+                 |
|               | Recipient is onboarded                          | Non-onboarded   |
|               v                                                 v                 |
|  +------------------------------------+        +-------------------------------+  |
|  |   ENTERPRISE MLS 1.0 TERMINATION   |        |   SMART FALLBACK ENGINE       |  |
|  |   - Encrypts against user device   |        |   - Generates signed Instant  |  |
|  |   - Wraps into Sealed Envelope     |        |     Web / App Clip Card URL   |  |
|  |   - Submits to sonaraim-node relay |        |     {BASE}/c/{token}          |  |
|  +------------------+-----------------+        +---------------+---------------+  |
|                     |                                          |                  |
|                     v                                          v                  |
|          +---------------------+                    +---------------------+       |
|          |    sonaraim-node    |                    | Instant Web Browser |       |
|          |    Relay Mailbox    |                    | & App Clip Preview  |       |
|          +---------------------+                    +---------------------+       |
|                                                                                   |
|  INBOUND USER CARD ACTIONS & ZERO-MARKUP PAYMENTS:                                |
|  - User taps interactive card button -> POST /v1/cards/actions (signed intent)    |
|  - Outbound Webhook Dispatcher forwards to Chatwoot/HubSpot CRM (webhooks.rs)    |
|  - Zero-markup Payment Intents -> POST /v1/payments/intents (billing.rs)          |
+-----------------------------------------------------------------------------------+
```

> [!NOTE]
> For full Mermaid sequence diagrams of WhatsApp conversion flows, consent token validation, and instant clip fallbacks, see [docs/DIAGRAMS.md](docs/DIAGRAMS.md).

---

## 📊 HTTP Endpoints Summary

| Method | Endpoint | Purpose |
| :--- | :--- | :--- |
| `GET` | `/health` | Health and upstream node connectivity status |
| `GET` | `/metrics` | Prometheus metrics scrape endpoint |
| `POST` | `/v1/tenants` | Dynamic enterprise tenant registration |
| `POST` | `/v1/tenants/rotate-key` | Rotate tenant Ed25519 signing keys |
| `POST` | `/v1/messages` | Dispatch E2EE message or declarative card |
| `POST` | `/v1/cards/actions` | Inbound signed user card action callback |
| `POST` | `/v1/consents/revoke` | Revoke user promotional consent token |
| `POST` | `/v1/payments/intents` | Create payment intent for zero-redirect card checkout |
| `POST` | `/v1/whatsapp-compat/{phone_id}/messages` | Meta WhatsApp Cloud API compatibility endpoint |
| `GET` | `/v1/tenants/{did}/analytics` | Tenant delivery volume and consent analytics |
| `GET` | `/v1/tenants/{did}/subscription` | Tenant seat limits and subscription status |
| `POST` | `/v1/integrations/crm/{provider}/events` | Inbound CRM integration webhook (Chatwoot, HubSpot, etc.) |

---

## 🧩 Repository Layout

```text
sonaraim-b2b/
├── docs/
│   ├── ARCHITECTURE.md          # Enterprise gateway subsystem design
│   ├── API.md                   # Complete REST & adapter API reference
│   ├── CONFIGURATION.md         # Environment variable reference
│   ├── DIAGRAMS.md              # Mermaid sequence and state diagrams
│   └── STANDARDS.md             # Coding, testing, and linting rules
├── src/
│   ├── billing.rs               # Flat-rate seat billing & zero-markup take rates
│   ├── config.rs                # Zero-hardcode runtime configuration
│   ├── consent.rs               # Cryptographic Consent Gate & token validation
│   ├── crm_connectors.rs        # Chatwoot, Zendesk, HubSpot, Salesforce connectors
│   ├── fallback.rs              # Smart Fallback Engine & Instant Clip URL generator
│   ├── http_api.rs              # Lightweight HTTP REST server & dispatch router
│   ├── lib.rs                   # Crate root and public exports
│   ├── main.rs                  # B2B daemon binary entry point
│   ├── outbound_dispatcher.rs   # Signed outbound webhook worker with backoff
│   ├── persistence.rs           # JSONL durable snapshot persistence
│   ├── routes.rs                # Endpoint request handlers & response encoders
│   ├── webhooks.rs              # Replay protection & signature validators
│   └── whatsapp_adapter.rs      # Meta WhatsApp Cloud API template translator
├── tests/
│   ├── b2b_test.rs              # Core consent, config, and fallback unit tests
│   ├── milestone3_test.rs       # CRM and HTTP API integration tests
│   ├── milestone6_b2b_test.rs   # Redis rate limiting & JSONL persistence tests
│   └── milestone12_b2b_test.rs  # WhatsApp E.164 normalization & rotation tests
├── Cargo.toml                   # Rust crate manifest (Apache-2.0)
├── CODE_OF_CONDUCT.md           # Contributor Covenant v2.1
├── CONTRIBUTING.md              # Development, TDD & git workflow guide
├── LICENSE                      # Apache License 2.0
└── SECURITY.md                  # Vulnerability disclosure & enterprise security
```

---

## 🚀 Quick Start

### Build & Test

```bash
# 1. Format check
cargo fmt --check

# 2. Static analysis
cargo clippy --all-targets -- -D warnings

# 3. Test execution
cargo test

# 4. Markdown linting
npx --yes markdownlint-cli2 "*.md" "docs/*.md"
```

---

## 🌐 Collaboration & Governance

- **[Contributing Guide](CONTRIBUTING.md)** — Branching model (`develop`), TDD mandate, and commit conventions.
- **[Code of Conduct](CODE_OF_CONDUCT.md)** — Community standards and enforcement guidelines.
- **[Security Policy](SECURITY.md)** — Vulnerability reporting, SLAs, and enterprise threat model.
- **[API Reference](docs/API.md)** — Complete REST, adapter, and connector documentation.
- **[Configuration Guide](docs/CONFIGURATION.md)** — Environment variable specifications and validation rules.
- **[Architecture Specification](docs/ARCHITECTURE.md)** — In-depth gateway design and ASCII layouts.
- **[System Diagrams](docs/DIAGRAMS.md)** — Mermaid sequence, topology, and state diagrams.

---

## 📜 License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE) for details.

Copyright 2026 Sonaraim Contributors.
