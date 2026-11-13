# Security Policy (`sonaraim-b2b`)

## 1. Supported Versions

Security updates and vulnerability patches are actively provided for the following releases:

| Version | Supported | Security Policy |
| :--- | :--- | :--- |
| `develop` | :white_check_mark: Yes | Bleeding edge; security fixes land here first |
| `0.1.x` | :white_check_mark: Yes | Current minor release series |
| `< 0.1.0` | :x: No | Unsupported pre-release builds |

---

## 2. Reporting a Vulnerability

The Sonaraim engineering team treats security issues with high urgency. We ask all security researchers and contributors to disclose findings responsibly.

### Reporting Procedure

- **Email**: Send vulnerability reports directly to [security@sonaraim.org](mailto:security@sonaraim.org).
- **PGP Encryption**: For proof-of-concept exploits, please encrypt your message with the Sonaraim Security PGP Key (`https://sonaraim.org/security.asc`).
- **Required Details**:
  - Summary of the vulnerability and affected route, adapter, or connector.
  - Reproducible test case, curl command, or payload.
  - Assessment of potential impact on tenant isolation, consent bypass, or key leakage.

### Response SLAs

- **Initial Acknowledgment**: Within **24 hours**.
- **Triage & Classification**: Within **48 hours**.
- **Remediation & Patch Target**: Critical severity issues are patched within **7 days**; high severity issues within **14 days**.
- **Coordinated Disclosure**: We request a **90-day embargo** from initial notification before public release.

---

## 3. Threat Model & Enterprise Security Scope

`sonaraim-b2b` acts as the secure perimeter between enterprise business applications and the decentralized Sonaraim messaging mesh.

### 3.1 Enterprise E2EE Bridge Termination

- **Cryptographic Enclave**: End users encrypt messages to verified enterprise DID keys. `sonaraim-b2b` terminates the MLS group session inside the enterprise's isolated trust boundary (or on-premise Sovereign Node).
- **Tenant Isolation**: Each enterprise tenant (`did:web`) has isolated cryptographic keyrings, seat limits, and webhook signing keys.
- **KMS Key Protection**: Tenant credentials and master secrets are protected with envelope encryption using `B2B_KMS_MASTER_KEY`.

### 3.2 Consent Gate & Anti-Spam Governance

- **Transactional vs. Promotional**: Transactional messages require verified `did:web:` identities. Promotional campaigns strictly require a valid, non-expired `ConsentToken` cryptographically signed by the user's device.
- **One-Tap Revocation**: When a user revokes consent via `POST /v1/consents/revoke`, subsequent promotional attempts are rejected immediately at the edge.

### 3.3 Outbound Webhooks & Inbound Replay Protection

- **Signed Webhooks**: Outbound card action and CRM event webhooks include timestamped Ed25519 or HMAC signatures in `X-Sonaraim-Signature`.
- **Replay Window**: Inbound webhooks enforce a strict timestamp replay window to prevent replay attacks.

### 3.4 WhatsApp Cloud API Adapter Security

- **Safe Translation**: Meta WhatsApp Business Cloud API payloads are translated into declarative `SonaraimCardDraft` objects without permitting executable code or HTML injection.

---

## 4. Out-of-Scope Vulnerabilities

- Compromise of external CRM provider API credentials (Chatwoot, Zendesk, HubSpot) outside `sonaraim-b2b`.
- Phishing or social engineering targeting merchant support staff.
- Distributed denial of service (DDoS) against public ingress mitigated by reverse proxy rate limiting.
