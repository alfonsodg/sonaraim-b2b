# Configuration Guide — `sonaraim-b2b`

> Zero-hardcode environment variable configuration reference for `sonaraim-b2b`.

All runtime settings in `sonaraim-b2b` are explicitly loaded from process environment variables. No default hostnames, port numbers, upstream URLs, or rate limits exist in the binary.

---

## 1. Core Runtime Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_HTTP_HOST` | `String` | Yes | Bind address for the HTTP REST & WhatsApp adapter server. | `0.0.0.0` |
| `B2B_HTTP_PORT` | `u16` | Yes | TCP port for the HTTP REST & WhatsApp adapter server (`1..=65535`). | `8042` |
| `B2B_GRPC_PORT` | `u16` | Yes | TCP port for the internal AI stream listener (`1..=65535`; plain TCP/JSON, not gRPC). | `8043` |
| `B2B_NODE_RPC_URL` | `String` | Yes | Upstream `sonaraim-node` RPC endpoint for native MLS E2EE dispatch. | `http://sonaraim-node:8040` |
| `B2B_INSTANT_CLIP_BASE_URL` | `String` | Yes | Base URL used by `SmartFallbackEngine` to build Instant Web / App Clip links. | `https://clip.sonaraim.io` |
| `B2B_PROMO_RATE_LIMIT_RPS` | `u32` | Yes | Maximum promotional messages per window per verified enterprise tenant (`>= 1`). | `50` |
| `B2B_STORAGE_PATH` | `String` | Yes | Filesystem path for durable JSONL tenant state snapshots (required). | `/var/lib/sonaraim/b2b-state.jsonl` |

---

## 2. Bootstrap & Security Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_BOOTSTRAP_TENANT_DID` | `String` | No | Pre-provisioned bootstrap tenant DID for initial bootstrapping. | `did:web:demo.sonaraim.org` |
| `B2B_BOOTSTRAP_API_KEY` | `String` | No | Pre-provisioned API key for the bootstrap tenant. | `sec_bootstrap_key_1234` |
| `B2B_KMS_MASTER_KEY` | `String` | No | Admin master key for `POST /v1/tenants` (`X-Sonaraim-Admin-Key`) and HMAC signing-secret derivation. | `master-admin-key-secret` |
| `B2B_REDIS_URL` / `REDIS_URL` | `String` | No | Redis connection URL for distributed promotional rate limiting. | `redis://:secret@redis:6379/1` |
| `B2B_PROMO_WINDOW_SECS` | `u64` | No | Rate-limit window length in seconds (default `1`). | `1` |

> Test-only: `B2B_TEST_NOW_OFFSET_SECS` (integer seconds, default `0`) shifts
> the wall clock used by the in-memory promotional rate limiter. It exists so
> `tests/issue20_window_test.rs` can advance the fixed window deterministically
> without sleeping; never set it in production.
>
> Roadmap: `POSTGRES_DSN` is not implemented (no PostgreSQL client exists in this crate).

---

## 3. AI Stream Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_AI_BACKEND_URL` | `String` | No | Pluggable inference backend URL (`POST {model, prompt}` -> `{"deltas":[...]}`). | `http://ai-backend:8080/v1/complete` |
| `B2B_AI_MODEL` | `String` | No | Model name sent to the inference backend (default `sonaraim-default`). | `sonaraim-1` |

---

## 4. Webhook & Integration Variables

| Variable | Type | Required | Description | Example |
| :--- | :--- | :--- | :--- | :--- |
| `B2B_WEBHOOK_TIMEOUT_MS` | `u64` | No | Timeout in milliseconds for outbound merchant webhook deliveries (default 350). | `350` |
| `B2B_WEBHOOK_REPLAY_SECS` | `u64` | No | Anti-replay window for `POST /v1/cards/actions` timestamps (default 300). | `300` |
| `B2B_ALLOW_INSECURE_CALLBACKS` | `bool` | No | Allow `http://` and loopback callback URLs (dev/test only, default `false`). | `false` |

---

## 5. Validation Rules

When `B2bConfig::from_env()` executes:

1. `B2B_HTTP_HOST`, `B2B_NODE_RPC_URL`, and `B2B_INSTANT_CLIP_BASE_URL` must be non-empty strings.
2. `B2B_HTTP_PORT` and `B2B_GRPC_PORT` must be valid non-zero `u16` integers (`1..=65535`).
3. `B2B_PROMO_RATE_LIMIT_RPS` must be a valid non-zero `u32` integer (`>= 1`).
4. `B2B_STORAGE_PATH` must be a non-empty string (required).
5. Trailing slashes in `B2B_INSTANT_CLIP_BASE_URL` are automatically stripped for canonical URL joining.
