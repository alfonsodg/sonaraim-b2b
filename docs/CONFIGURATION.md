# Configuration Guide — `sonaraim-b2b`

> Zero-hardcode environment variable configuration reference for `sonaraim-b2b`.

## Environment Variables

All runtime settings are mandatory and loaded via `B2bConfig::from_env()`. No default values are hardcoded in the source code.

| Variable | Type | Example | Description |
| :--- | :--- | :--- | :--- |
| `B2B_HTTP_HOST` | `String` | `0.0.0.0` | Bind address for the HTTP REST & WhatsApp adapter server. |
| `B2B_HTTP_PORT` | `u16` | `8040` | TCP port for the HTTP REST & WhatsApp adapter server. |
| `B2B_GRPC_PORT` | `u16` | `8041` | TCP port for the internal gRPC enterprise bridge. |
| `B2B_NODE_RPC_URL` | `String` | `http://sonaraim-node:8031` | Upstream `sonaraim-node` RPC endpoint for native MLS E2EE dispatch. |
| `B2B_INSTANT_CLIP_BASE_URL` | `String` | `https://clip.sonaraim.io` | Base URL used by `SmartFallbackEngine` to build Instant Web / App Clip links. |
| `B2B_PROMO_RATE_LIMIT_RPS` | `u32` | `50` | Maximum promotional messages per second per verified enterprise tenant. |

## Validation Rules

- `B2B_HTTP_HOST`, `B2B_NODE_RPC_URL`, and `B2B_INSTANT_CLIP_BASE_URL` must be non-empty strings.
- `B2B_HTTP_PORT` and `B2B_GRPC_PORT` must be valid non-zero `u16` integers (`1..=65535`).
- `B2B_PROMO_RATE_LIMIT_RPS` must be a valid non-zero `u32` integer (`>= 1`).
