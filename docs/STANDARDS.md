# Development Standards — `sonaraim-b2b`

> Engineering standards, code quality gates, and testing conventions for the Sonaraim Enterprise B2B Bridge.

## 1. Language & Toolchain

- **Language**: Latest stable Rust (2024 edition).
- **Package Manager**: `cargo`.
- **Zero-Hardcode Policy**: All network addresses, ports, upstream URLs, and rate limits must be injected through environment variables loaded by `B2bConfig::from_env()`.

## 2. Code Organization & Modularity

- Every source file must remain strictly under **300 lines**.
- Each module encapsulates a single domain responsibility:
  - `src/config.rs`: Environment configuration parsing and validation.
  - `src/consent.rs`: Cryptographic Consent Gate and `did:web` verification enforcement.
  - `src/whatsapp_adapter.rs`: 1-Click WhatsApp Business Cloud API template translation.
  - `src/fallback.rs`: Smart Fallback decision engine for native MLS E2EE vs Instant Web / App Clip magic links.
- Every struct, enum, method, and function must include complete Rustdoc comments (`///` or `//!`) describing purpose, parameters, return values, and error conditions.

## 3. Test-Driven Development (TDD) & Quality Gates

- All features and bug fixes follow **Red -> Green -> Refactor**.
- Pre-commit quality gates:
  - `cargo fmt --check`
  - `cargo clippy -- -D warnings`
  - `cargo test`
  - `markdownlint README.md REMOTE.md docs/*.md`

## 4. Git & Issue Workflow

- Every commit follows Conventional Commits (`<type>(<scope>): <subject> (#issue)`) and includes `Ref #N` in the footer.
- All changes originate from a tracked GitHub issue with `priority::*` and `type::*` labels.
