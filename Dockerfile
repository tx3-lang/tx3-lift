# ── Builder ─────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /build

COPY . .

RUN cargo build --release -p tracker

# ── Runtime ─────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# ca-certificates is required for TLS validation against the upstream gRPC endpoint.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /build/target/release/tracker /usr/local/bin/tracker

# No config, TII, or secret is baked in.
# Provide tracker.toml (and any TII files) via bind-mount at run time.
# Pass the config path as the container CMD, e.g.:
#   docker run ... tracker:dev /etc/tracker/tracker.toml
ENTRYPOINT ["/usr/local/bin/tracker"]
