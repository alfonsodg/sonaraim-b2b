# API Reference — `sonaraim-b2b`

> REST, Adapter, and Rust library interfaces exposed by `sonaraim-b2b`.

## 1. 1-Click WhatsApp Cloud API Compatibility Endpoint

### `POST /v1/whatsapp/messages`

Accepts a legacy WhatsApp Business Cloud API template request and translates it into a native `SonaraimCardDraft`.

#### Request Payload (`WhatsAppTemplateRequest`)

```json
{
  "messaging_product": "whatsapp",
  "to": "did:sonaraim:alice",
  "template_name": "order_confirmation",
  "language_code": "en_US",
  "category": "TRANSACTIONAL",
  "body_parameters": ["ORD-9941", "$42.50"],
  "buttons": ["Track Package", "Contact Support"]
}
```

#### Translated Output (`SonaraimCardDraft`)

```json
{
  "recipient": "did:sonaraim:alice",
  "title": "template:order_confirmation (en_US)",
  "body": "ORD-9941 | $42.50",
  "actions": ["Track Package", "Contact Support"],
  "category": "Transactional"
}
```

## 2. Consent Gate Verification

### `ConsentGate::authorize_dispatch`

| Parameter | Type | Description |
| :--- | :--- | :--- |
| `tenant_did` | `&str` | Enterprise decentralized identifier (must start with `did:web:`). |
| `did_verified` | `bool` | Whether the enterprise domain DNS/HTTPS `.well-known/did.json` proof passed. |
| `recipient_did` | `&str` | Target recipient identifier. |
| `category` | `MessageCategory` | `Transactional` or `Promotional`. |
| `consent_token` | `Option<&ConsentToken>` | Cryptographic user opt-in token (required for `Promotional`). |
| `now_unix_secs` | `u64` | Current Unix timestamp in seconds for expiration evaluation. |

## 3. Smart Fallback Engine

### `SmartFallbackEngine::resolve_route`

Determines delivery route based on recipient onboarding status:

- Returns `DeliveryRoute::NativeMlsE2ee { node_rpc_url, recipient_did }` when `recipient_onboarded == true`.
- Returns `DeliveryRoute::InstantClip { magic_link_url, clip_token }` formatted as `{B2B_INSTANT_CLIP_BASE_URL}/c/{clip_token}` when `recipient_onboarded == false`.
