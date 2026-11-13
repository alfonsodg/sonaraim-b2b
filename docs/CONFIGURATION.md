# Configuration Guide — `sonaraim-b2b`

> Zero-hardcode environment variable configuration reference for `sonaraim-b2b`.

All runtime settings in `sonaraim-b2b` are explicitly loaded from process environment variables. No default hostnames, port numbers, cryptographic keys, or rate limits exist in the binary.

---

## 1. Core Runtime Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_HTTP_HOST` | `String` | Yes | Bind address for the HTTP REST & WhatsApp adapter server. | `0.0.0.0` |
| `B2B_HTTP_PORT` | `u16` | Yes | TCP port for the HTTP REST & WhatsApp adapter server (`1..=65535`). | `8042` |
| `B2B_GRPC_PORT` | `u16` | Yes | TCP port for the internal gRPC enterprise bridge (`1..=65535`). | `8043` |
| `B2B_NODE_RPC_URL` | `String` | Yes | Upstream `sonaraim-node` RPC endpoint for native MLS E2EE dispatch. | `http://sonaraim-node:8040` |
| `B2B_INSTANT_CLIP_BASE_URL` | `String` | Yes | Base URL used by `SmartFallbackEngine` to build Instant Web / App Clip links. | `https://clip.sonaraim.io` |
| `B2B_PROMO_RATE_LIMIT_RPS` | `u32` | Yes | Maximum promotional messages per second per verified enterprise tenant (`>= 1`). | `50` |

---

## 2. Bootstrap & Security Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_BOOTSTRAP_TENANT_DID` | `String` | No | Pre-provisioned bootstrap tenant DID for initial bootstrapping. | `did:web:demo.sonaraim.org` |
| `B2B_BOOTSTRAP_API_KEY` | `String` | No | Pre-provisioned API key for the bootstrap tenant. | `sec_bootstrap_key_1234` |
| `B2B_ADMIN_API_KEY` | `String` | No | Master administrator API key for provisioning new tenants via `POST /v1/tenants`. | `master-admin-key-secret` |
| `B2B_KMS_MASTER_KEY` | `String` | No | KMS master key URI or secret used to encrypt tenant signing credentials at rest. | `arn:aws:kms:us-east-1:123456789012:key/...` |

---

## 3. Storage, Queue & Database Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_STORAGE_PATH` | `String` | No | Filesystem path for durable JSONL tenant state and message snapshot persistence. | `/var/lib/sonaraim/b2b-state.jsonl` |
| `REDIS_URL` | `String` | No | Redis connection URL for distributed rate limiting and deduplication. | `redis://:secret@redis:6379/1` |
| `POSTGRES_DSN` | `String` | No | PostgreSQL connection string for relational enterprise metadata and billing. | `postgres://sonaraim:secret@postgres:5432/sonaraim_b2b` |

---

## 4. Webhook & Integration Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_WEBHOOK_TIMEOUT_MS` | `u64` | No | Timeout in milliseconds for outbound merchant webhook deliveries (default 5000). | `5000` |

---

## 5. Validation Rules

When `B2bConfig::from_env()` executes:

1. `B2B_HTTP_HOST`, `B2B_NODE_RPC_URL`, and `B2B_INSTANT_CLIP_BASE_URL` must be non-empty strings.
2. `B2B_HTTP_PORT` and `B2B_GRPC_PORT` must be valid non-zero `u16` integers (`1..=65535`).
3. `B2B_PROMO_RATE_LIMIT_RPS` must be a valid non-zero `u32` integer (`>= 1`).
4. Trailing slashes in `B2B_INSTANT_CLIP_BASE_URL` are automatically stripped for canonical URL joining.
