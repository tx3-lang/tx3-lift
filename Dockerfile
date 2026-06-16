# ── Builder ─────────────────────────────────────────────────────────────────
FROM rust:latest AS builder

WORKDIR /build

# Copy workspace manifests first so Cargo can fetch deps before the source
# (layer-cache friendly; change to source only recompiles, not downloads).
COPY Cargo.toml ./
COPY crates/tx3-lift/Cargo.toml         crates/tx3-lift/Cargo.toml
COPY crates/tx3-lift-cardano/Cargo.toml crates/tx3-lift-cardano/Cargo.toml
COPY bin/tracker/Cargo.toml             bin/tracker/Cargo.toml

# Seed stub libs so `cargo fetch` / dependency compilation works without real src.
RUN mkdir -p crates/tx3-lift/src \
             crates/tx3-lift-cardano/src \
             bin/tracker/src \
 && echo 'fn main() {}' > bin/tracker/src/main.rs \
 && echo '' > crates/tx3-lift/src/lib.rs \
 && echo '' > crates/tx3-lift-cardano/src/lib.rs

# Pre-compile all deps (cached as long as Cargo.toml files do not change).
RUN cargo build --release -p tracker 2>&1 | tail -1 || true
# Remove stub artifacts so the real build does not skip recompilation.
RUN rm -rf target/release/.fingerprint/tracker* \
           target/release/.fingerprint/tx3_lift* \
           target/release/deps/tracker* \
           target/release/deps/tx3_lift*

# Copy the full source and build the real binary.
COPY crates/ crates/
COPY bin/    bin/

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
