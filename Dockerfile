# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder
WORKDIR /usr/src/sonaraim-b2b

COPY Cargo.toml ./
COPY src ./src
COPY tests ./tests

RUN cargo test --release && cargo build --release

FROM debian:bookworm-slim AS runtime
RUN groupadd -r sonaraim && useradd -r -g sonaraim -s /sbin/nologin sonaraim
WORKDIR /app

COPY --from=builder /usr/src/sonaraim-b2b/target/release/sonaraim-b2b /usr/local/bin/sonaraim-b2b

USER sonaraim
EXPOSE 8040 8041
ENTRYPOINT ["/usr/local/bin/sonaraim-b2b"]
