# Sonaraim B2B Enterprise Bridge

> Zero-Knowledge Enterprise E2EE Bridge, 1-Click WhatsApp Cloud API Adapter, Cryptographic Consent Gate, and Smart Fallback Engine

`sonaraim-b2b` enables verified enterprises (`did:web`) to migrate from high-cost, unencrypted legacy channels (WhatsApp Business Cloud API and SMS OTPs) to end-to-end encrypted, rich interactive `SonaraimCard` workflows with zero infrastructure lock-in.

## Core Capabilities

- **Zero-Hardcode Configuration (`config`)**: Loads all network bindings, node RPC endpoints, Instant Clip base URLs, and rate-limit thresholds strictly from environment variables.
- **Cryptographic Consent Gate (`consent`)**: Enforces strict separation between `Transactional` and `Promotional` traffic. Verified `did:web` tenants can dispatch unlimited transactional notifications (OTPs, receipts, fraud alerts, flight updates), while promotional dispatches require an unrevoked, non-expired user `ConsentToken`.
- **1-Click WhatsApp Business Cloud API Adapter (`whatsapp_adapter`)**: Translates legacy Meta Graph API `/v18.0/{phone-number-id}/messages` template payloads directly into native declarative `SonaraimCardDraft` envelopes without rewriting enterprise webhooks.
- **Smart Fallback Engine (`fallback`)**: Routes payloads over native MLS E2EE when the recipient is registered on Sonaraim, or generates a signed Instant Web / App Clip magic link (`{B2B_INSTANT_CLIP_BASE_URL}/c/{token}`) when the recipient has not yet installed the client.

## Architecture & Documentation

- [Development Standards](docs/STANDARDS.md)
- [System Architecture](docs/ARCHITECTURE.md)
- [Mermaid Diagrams](docs/DIAGRAMS.md)
- [API Reference](docs/API.md)
- [Configuration Guide](docs/CONFIGURATION.md)

## Quick Start

```bash
cp samples/.env.example .env
set -a && source .env && set +a
cargo test
cargo run
```
