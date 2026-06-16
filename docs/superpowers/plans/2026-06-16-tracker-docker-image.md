# Tracker Docker image + release binary — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish a generic, secret-free Docker image of the tracker to GHCR (multi-arch) plus a standalone musl release binary, and let the tracker read its API key from `DMTR_API_KEY`.

**Architecture:** A small Rust change adds an env fallback for the API key (pure resolver + thin env read), so configs and images ship without secrets. A multi-stage Dockerfile produces a slim runtime image with the binary + CA certs. Two GitHub Actions workflows publish the image (buildx, amd64+arm64) and the binary (musl) on tag.

**Tech Stack:** Rust, `cargo`, Docker buildx, GitHub Actions, GHCR.

**Spec:** `docs/superpowers/specs/2026-06-16-tracker-docker-image-design.md`

**Cross-repo dependency:** The dashboard compose stack (companion plan) consumes the image and the env-fallback behaviour produced here. Land Task 1 before that compose can ship a secret-free `deploy/tracker.toml`.

**Conventions for this plan:** Per the team's code-free-plan convention, steps state intent, exact files, and verification commands/expected output — not source content. Authoring follows the spec sections referenced in each task.

---

### Task 1: API key env fallback (Path B)

**Files:**
- Modify: `bin/tracker/src/config.rs` (add a pure resolver + call site)
- Modify: `bin/tracker/src/main.rs` (read `DMTR_API_KEY`, pass to resolver) — only if the env read lands here rather than in `config::load`
- Test: `bin/tracker/src/config.rs` (`#[cfg(test)]` module, alongside existing config tests)

- [ ] **Step 1: Write failing unit tests for a pure `resolve_api_key(toml: Option<String>, env: Option<String>) -> Option<String>`** covering: env used when toml is `None`; toml wins when both present; `None` when both absent. Keep it a pure function (no `std::env` inside) so tests are deterministic and parallel-safe.

- [ ] **Step 2: Run the tests, confirm they fail** — `cargo test -p tracker resolve_api_key`. Expected: FAIL (function not defined).

- [ ] **Step 3: Implement `resolve_api_key`** as a pure function in `config.rs`, then wire it at the real call site (after `config::load`, reading `std::env::var("DMTR_API_KEY").ok()`) so `cfg.upstream.api_key` is filled from env when the TOML omits it. Per spec §4.2: explicit TOML value wins; both-absent does not panic.

- [ ] **Step 4: Run tests, confirm they pass** — `cargo test -p tracker`. Expected: PASS (new + existing config tests).

- [ ] **Step 5: Sanity-check the wiring** — `cargo build -p tracker`, then run against a throwaway TOML with no `api_key` and `DMTR_API_KEY` unset; confirm it starts (logs "starting tracker") and does not panic. Expected: starts, then a recoverable connect/stream error from the upstream (spec criterion T4).

- [ ] **Step 6: Commit** — `git add bin/tracker/src/config.rs bin/tracker/src/main.rs && git commit`.

---

### Task 2: Document the env fallback

**Files:**
- Modify: `bin/tracker/tracker.toml.example` (comment near `api_key` noting the `DMTR_API_KEY` fallback)
- Modify: `README.md` (tracker repo) if it documents config/auth

- [ ] **Step 1:** Update the `api_key` comment in `tracker.toml.example` to state the key may be supplied via `DMTR_API_KEY` instead of the TOML.
- [ ] **Step 2:** Update any README mention of how the key is provided to match Task 1.
- [ ] **Step 3: Verify** the example file still parses by pointing the binary at a copy of it (it should fail only on a missing real upstream/sources, not on a TOML parse error).
- [ ] **Step 4: Commit.**

---

### Task 3: Secret hygiene — scrub committed key

**Files:**
- Modify: `tracker.toml` (repo root) — remove the inline `api_key` value
- Verify: no other committed file carries a live `utxorpc…`/`dmtr_…` key

- [ ] **Step 1: Grep the repo** for committed keys — `git grep -nE 'api_key\s*=\s*"(utxorpc|dmtr_)'`. Record every hit.
- [ ] **Step 2:** Remove/blank the inline `api_key` from `tracker.toml` so the file relies on `DMTR_API_KEY`.
- [ ] **Step 3: Re-grep**, expected: no hits.
- [ ] **Step 4: Flag rotation** — the previously committed key is in git history and must be **rotated** in Demeter (out-of-band, human action). Note it in the PR description; this step cannot be completed in code.
- [ ] **Step 5: Commit.**

---

### Task 4: Tracker Dockerfile

**Files:**
- Create: `Dockerfile` (repo root)
- Create: `.dockerignore` (repo root)

- [ ] **Step 1: Author the multi-stage Dockerfile** per spec §4.1: builder on `rust` running `cargo build --release -p tracker`; runtime on `debian:bookworm-slim` with `ca-certificates`; copy only the binary; entrypoint runs the binary with the config path as its argument (default `argv[1]`). No config, TII, or secret baked in. `.dockerignore` excludes `target/`, `*.db*`, `.git`.
- [ ] **Step 2: Build the image** — `docker build -t tracker:dev .`. Expected: success.
- [ ] **Step 3: Run with a mounted minimal config and `DMTR_API_KEY` set** — mount a TOML + a TII, point `database_path` at a writable mounted dir. Expected: starts, logs "starting tracker", and (with a real key) writes the DB; (without a real key) connects and surfaces the upstream rejection — no crash on missing CA certs (proves `ca-certificates` is present).
- [ ] **Step 4: Confirm no secret in the image** — `docker history` / inspect the build context; confirm no key and no baked config.
- [ ] **Step 5: Commit.**

---

### Task 5: CI workflow — publish image to GHCR (multi-arch)

**Files:**
- Create: `.github/workflows/docker-tracker.yml`

- [ ] **Step 1: Author the workflow** per spec §4.4: trigger on tag/release; `docker/setup-buildx-action`; login to GHCR with the workflow token; build for `linux/amd64,linux/arm64`; push to `ghcr.io/tx3-lang/tracker` with tags = released version, commit SHA, and `latest`; `permissions: packages: write`.
- [ ] **Step 2: Validate workflow syntax** — `actionlint .github/workflows/docker-tracker.yml` (or equivalent). Expected: no errors.
- [ ] **Step 3: Dry-run the build matrix locally** — `docker buildx build --platform linux/amd64,linux/arm64 .` (no push). Expected: both arches build (arm64 via QEMU may be slow).
- [ ] **Step 4: Commit.**
- [ ] **Step 5: Post-merge verification (manual):** push a tag, confirm the Actions run publishes the image and the GHCR package is **public** (spec T1).

---

### Task 6: CI workflow — release binary (musl)

**Files:**
- Create: `.github/workflows/release-binary.yml`

- [ ] **Step 1: Verify the musl target builds locally** — build `tracker` for `x86_64-unknown-linux-musl` (via `cross` or a musl toolchain). Expected: a static binary; confirm `rusqlite` bundled compiles under musl.
- [ ] **Step 2: Smoke-test the binary** — run it against a throwaway TOML on a host; expected: starts and reads config (proves it's self-contained).
- [ ] **Step 3: Author the workflow** per spec §4.3: on tag, build the musl target(s) (x86_64, optionally aarch64) and upload the artifact(s) to the GitHub Release.
- [ ] **Step 4: Validate workflow syntax** — `actionlint`. Expected: no errors.
- [ ] **Step 5: Commit.**
- [ ] **Step 6: Post-merge verification (manual):** tag a release, confirm the binary is downloadable and runs on a clean host (spec T5).

---

### Task 7: Final verification & PR

- [ ] **Step 1:** Re-run `cargo test -p tracker` and `cargo build --release -p tracker`. Expected: green.
- [ ] **Step 2:** Walk the spec acceptance criteria T1–T5; confirm each maps to a completed task (T1/T5 have manual post-merge steps).
- [ ] **Step 3:** Open a PR from `docs/docker-deploy-spec` (or a fresh feature branch) summarising the image, binary, env fallback, and the **key-rotation** action item.

---

## Self-review notes

- **Spec coverage:** §4.1→Task 4; §4.2→Task 1+2; §4.3→Task 6; §4.4→Task 5; §6 (secret hygiene)→Task 3; T1/T5 manual steps flagged.
- **Cross-repo:** Task 1 is the dependency for the dashboard plan's secret-free `deploy/tracker.toml`.
- **TDD note:** Only Task 1 is unit-testable; infra tasks (4–6) use build-and-run verification, which is the honest analog for Dockerfiles/CI.
