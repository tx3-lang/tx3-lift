# Tracker Docker image + release binary — design spec

- **Date**: 2026-06-16
- **Repo**: `tx3-lang/tx3-lift`
- **Milestone**: Catalyst — "Publicly available Docker images for installing the complete monitoring system"
- **Status**: design approved, plan pending
- **Companion spec**: `tx3-lang/dashboard` → `2026-06-16-dashboard-docker-compose-design.md` (the compose stack that consumes this image)

---

> **Convention update (post-implementation):** to match the txpipe GHCR pattern,
> the published image is `ghcr.io/tx3-lang/tx3-lift-tracker` (named
> `<repo>-<component>`), tagged with the **git SHA only** (no semver / `latest`),
> and the publish workflow runs on `workflow_dispatch`. Inline references below to
> `ghcr.io/tx3-lang/tracker` and to semver/`latest` tags predate this alignment.

## 1. Context

### 1.1 What we have

- `bin/tracker` is a Rust binary that subscribes to a Cardano `utxorpc` stream, matches incoming transactions against TII protocol definitions, and writes one row per match into a local SQLite file (`tracker.db`, WAL mode).
- Configuration is a single TOML file passed as `argv[1]` (`main.rs:35`, default `tracker.toml`). It carries the upstream endpoint, the `api_key`, the storage `database_path`, and one or more `[[sources]]` each pointing at a `tii_path`.
- `RUST_LOG` is the only environment variable the binary reads today (via `EnvFilter`).
- The `orcfax-burn` example wraps the binary in a `run.sh` that splices `DMTR_API_KEY` from the environment into the TOML at runtime.

### 1.2 Catalyst requirement

- **A** — Publicly available Docker images for installing the complete monitoring system.
- **A1** — The Docker images allow the monitoring system to be installed and run successfully without additional configuration.

The "complete monitoring system" is tracker + dashboard. This spec covers the **tracker half**: a published Docker image and a downloadable binary. The dashboard image and the `docker-compose.yml` that wires the two together live in the companion spec.

### 1.3 Audience

dApp builders / operators who run the tracker as the data-producing sidecar of the monitoring system, plus non-Docker users who want a standalone binary.

---

## 2. Scope

### 2.1 In scope

1. A multi-stage **Docker image** for the tracker, published to **GHCR** (`ghcr.io/tx3-lang/tracker`), multi-arch (amd64 + arm64).
2. A **release binary** (static musl) attached to GitHub Releases.
3. A small **code change**: read the API key from `DMTR_API_KEY` when it is absent from the TOML (Path B), so neither the image nor the committed config carries a secret.
4. **CI workflows** that build/publish the image and the binary on tag/release.
5. **Secret hygiene**: ensure no live `api_key` ships in any committed config or image.

### 2.2 Out of scope (YAGNI)

- The dashboard image and the `docker-compose.yml` (companion spec, dashboard repo).
- An all-in-one image that bundles tracker + dashboard.
- Postgres storage backend.
- systemd / pm2 service units.

---

## 3. Decisions (locked during brainstorming)

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Registry | GHCR under `tx3-lang` | Integrates with GitHub Actions, no extra account/credentials, public pulls. |
| Image shape | **Generic** — binary + entrypoint only | No baked config, no baked secret. Reusable for any protocol; config is injected at run time. |
| Architectures | amd64 + arm64 (buildx) | Apple-Silicon operators. Droppable to amd64-only if build time is a problem. |
| API key | **Path B** — env fallback in the binary | Aligns `running.md` with reality, benefits non-Docker users, lets configs ship without secrets. |
| Binary | static `x86_64-unknown-linux-musl` (aarch64 optional) | Self-contained escape hatch for non-Docker users. |

---

## 4. Component design

### 4.1 Docker image

Multi-stage build:

- **Builder stage** on the official `rust` image: `cargo build --release -p tracker`. `rusqlite` is built with the `bundled` feature, which compiles SQLite from C source — the C toolchain in the `rust` image already satisfies this.
- **Runtime stage** on `debian:bookworm-slim` plus `ca-certificates`. The CA bundle is required: the upstream connection to Demeter is gRPC over TLS, and the client validates the server certificate against the system trust store.
- The image contains only the binary and a thin entrypoint. No TOML, no TII, no key.

**Run contract** (what the image expects, consumed by the compose spec):

- The TOML config is provided at run time (bind mount) and named as the first argument (`CMD`/args).
- TII files are provided at run time (bind mount).
- `database_path` in the TOML points at a writable path on a mounted volume.
- `DMTR_API_KEY` is supplied as an environment variable.
- `RUST_LOG` optionally tunes log level.

### 4.2 API key via environment (Path B)

Today `Config.upstream.api_key: Option<String>` is populated only from the TOML (`config.rs:44`). Change: after the config is loaded, if `api_key` is `None`, fall back to the `DMTR_API_KEY` environment variable. An explicit value in the TOML still wins.

Consequences:
- The committed `tracker.toml.example` keeps `api_key` commented out (already the case) and documents the env fallback.
- The repo's own `tracker.toml` must not carry a live key.
- `running.md` (dashboard repo) becomes accurate without a wrapper script.

Behaviour to cover with tests: env used when the TOML omits the key; TOML value takes precedence when present. With both absent the tracker still starts and connects without an auth header — today the upstream (Demeter) rejects the unauthenticated stream, surfacing as a connect/stream error, not a panic. Whether to additionally fail fast with a clear "no API key" message when both sources are empty is an open question (§7).

### 4.3 Release binary

A static `x86_64-unknown-linux-musl` build attached to GitHub Releases (optionally `aarch64` too). `rusqlite` bundled compiles cleanly under musl. Runtime TLS validation reads the host's certificate store, which is present on normal Linux hosts.

### 4.4 CI workflows

- **Image workflow**: on tag / release, `docker buildx` for amd64 + arm64, push to `ghcr.io/tx3-lang/tracker`. Tags: the released version, the commit SHA, and `latest`. Needs `packages: write` permission and GHCR login via the workflow token. The package is made public.
- **Binary workflow**: on tag, build the musl target(s) and upload the artifact(s) to the GitHub Release.

---

## 5. Acceptance criteria

| ID | Criterion |
|----|-----------|
| T1 | The image builds in CI for amd64 + arm64 and is published, public, on `ghcr.io/tx3-lang/tracker`. |
| T2 | `docker run` with a mounted TOML + TII and `DMTR_API_KEY` set connects upstream and writes `tracker.db` at the configured path. |
| T3 | No live `api_key` is present in the published image or in any committed config file. |
| T4 | With `api_key` absent from the TOML and `DMTR_API_KEY` set, the tracker authenticates; with both absent it does not panic (it connects unauthenticated and the upstream rejection surfaces as a recoverable/fatal stream error). |
| T5 | The musl binary is downloadable from a GitHub Release and runs against a TOML config on a clean host. |

---

## 6. Secret hygiene

- The repo's `tracker.toml` and any committed example currently visible with an inline `api_key` (an `utxorpc…` key) must be scrubbed; the key should be **rotated** since it is already in git history.
- Going forward, configs ship without keys; the key arrives via `DMTR_API_KEY`.

---

## 7. Risks & open points

- **musl + native certs**: the standalone binary relies on the host certificate store at runtime; documented as a prerequisite. The Docker image handles this with `ca-certificates`.
- **arm64 build time**: cross-building Rust under buildx/QEMU is slow; if it becomes a bottleneck, drop to amd64-only or use native arm runners.
- **Image size**: `debian:bookworm-slim` keeps it modest; distroless is a later optimisation, not a requirement.
- **Fail-fast on missing key (open)**: today a missing key only fails when the upstream rejects the stream. We may want an explicit startup check ("no API key in TOML or `DMTR_API_KEY`") for a clearer operator error. Decide during planning.

---

## 8. Dependencies

- The companion dashboard spec's `docker-compose.yml` consumes `ghcr.io/tx3-lang/tracker` and depends on the **Path B** env-fallback behaviour defined here (§4.2) to inject the key without a baked secret.
