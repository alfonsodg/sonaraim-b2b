# Contributing to Sonaraim B2B Gateway

Thank you for contributing to `sonaraim-b2b`! This repository implements the Sonaraim Enterprise E2EE Bridge, 1-Click WhatsApp Cloud API Adapter, Cryptographic Consent Gate, Smart Fallback Engine, and CRM integration connectors.

To ensure enterprise-grade stability, zero-trust cryptographic guarantees, and developer ergonomics, all contributions must adhere to the standards outlined below.

---

## 1. Branching & Git Workflow

- **`develop` Branch**: The primary integration branch. All feature and bugfix work must branch from and merge into `develop`.
- **`main` Branch**: Reserved strictly for production releases (`vMAJOR.MINOR.PATCH`). Never push or merge directly to `main`.
- **Branch Naming**:
  - Features: `feature/<feature-name>` (e.g., `feature/crm-connector-hubspot`)
  - Fixes: `fix/<issue-description>` (e.g., `fix/consent-token-expiry`)
- **Pull Requests**: Target `develop`. Every PR must pass all CI/CD quality gates and include test coverage.

---

## 2. Test-Driven Development (TDD) Mandatory

All development in `sonaraim-b2b` must strictly follow Test-Driven Development:

1. **Red**: Write a failing integration test or unit test covering the defect or new feature before touching production code.
2. **Green**: Write the minimal code necessary to satisfy the test assertions.
3. **Refactor**: Clean up and optimize while keeping all test suites green.
4. **Mandatory Coverage**: All HTTP routes, webhook signature verifications, CRM payloads, rate limits, and fallback paths must have automated test coverage.

---

## 3. Code Quality & Modularity Standards

- **File Length Limit**: Every source file must remain under 300 lines. Split large modules into focused single-responsibility files.
- **Mandatory Documentation**: Every public struct, enum, function, and method must have Rustdoc comments (`///`) describing its behavior, parameters, return values, and failure modes. Complex logic blocks must have inline comments explaining *why*.
- **Zero-Hardcode Policy**: Never hardcode URLs, IP addresses, port numbers, secrets, or rate limits. All settings must load via `B2bConfig::from_env()` or explicit environment variables (`B2B_BOOTSTRAP_TENANT_DID`, `B2B_BOOTSTRAP_API_KEY`, `B2B_KMS_MASTER_KEY`, etc.).
- **Error Handling**: Do not use `unwrap()` or `expect()` in request handlers or production execution paths. Always return typed errors with clear JSON response bodies.

---

## 4. Conventional Commits

All commit messages must follow the Conventional Commits specification and reference an issue number:

```text
<type>(<scope>): <subject> (Ref #N)
```

- **Types**: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`.
- **Scopes**: `adapter`, `consent`, `fallback`, `crm`, `billing`, `webhook`, `http`, `routes`.
- **Issue Reference**: Always reference the tracked issue using `Ref #N`.

Example:

```text
feat(crm): add HubSpot deal stage webhook and payment intent route (Ref #16)
```

---

## 5. Tooling, Verification & Quality Gates

Run and verify the following commands before committing:

```bash
# 1. Format verification
cargo fmt --check

# 2. Static analysis and linter
cargo clippy --all-targets -- -D warnings

# 3. Test execution
cargo test

# 4. Markdown linting
npx --yes markdownlint-cli2 "*.md" "docs/*.md"
```

All 4 commands must pass with zero errors.

---

## 6. Documentation & Architecture Standards

- **Language**: English only for code, documentation, comments, and commit messages.
- **Mermaid Diagrams**: Mermaid diagrams belong **exclusively** inside `docs/DIAGRAMS.md`. Never embed ```` ```mermaid ```` code blocks in `README.md` or `ARCHITECTURE.md` (use text or ASCII art instead).
- **Markdown Linting**: All markdown documents must pass `markdownlint` without errors.
