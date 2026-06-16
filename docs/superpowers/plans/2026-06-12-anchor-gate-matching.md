# Anchor Gate + Scored Matches Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the tracker from matching unrelated txs against every configured protocol, by gating each source on profile-derived anchors and recording score/rank on every match row.

**Architecture:** A new chain-neutral `ProtocolAnchors` type in `tx3-lift` derives anchor sets (party addresses, env UTxO refs, env policy ids) straight from a TII profile, bypassing the TIR expression tree. The tracker builds anchors per source at startup (disabling anchor-less sources with a warning), gates every streamed tx on anchor intersection, scores surviving candidates, dedups within a source, dense-ranks across sources, and optionally keeps only the top rank.

**Tech Stack:** Rust workspace; `tx3-lift` (chain-neutral lib), `tracker` binary (rusqlite + tokio), `tx3-sdk` TII types, `bech32`.

**Spec:** `docs/superpowers/specs/2026-06-12-anchor-gate-matching-design.md` — read it fully before starting. It records the decisions (row semantics, zero-anchor handling, scope) and the rationale; this plan does not repeat all of it.

**Per the user's explicit request, this plan contains NO implementation code.** Each task gives goals, behavior contracts, file pointers, and acceptance criteria; the implementer writes their own code. TDD ordering still applies: write the failing tests from the acceptance criteria first, watch them fail, then implement.

**Conventions:** conventional-commit messages (`feat:`, `test:`, `docs:`, `chore:` — see `git log --oneline`). Run `cargo fmt` and `cargo clippy --workspace --all-targets` before every commit.

---

## Context for the implementer (read first, ~10 min)

- `crates/tx3-lift/src/fingerprint.rs` — `Fingerprint`, `information_score()`, `PayloadSummary`-based pre-filter. The anchors module mirrors its style and sits beside it.
- `crates/tx3-lift/src/payload.rs:13-31` — `PayloadSummary` field list and `UtxoRef` type alias; anchors are checked against these sets.
- `crates/tx3-lift/src/specialize.rs:67-73` — `decode_bech32_address`, reuse for party decoding (do not re-implement).
- `bin/tracker/src/specialization.rs` — `SpecializedTii` cache built at startup; this is where anchors get attached and anchor-less sources get dropped.
- `bin/tracker/src/process.rs:120-161` — `run_specializations`, the loop being restructured; `apply_tx` (line 20) is its only caller, called from `main.rs`.
- `bin/tracker/src/store.rs` — `MIGRATIONS` list pattern, `MatchRow`/`OwnedMatchRow`, the `INSERT OR IGNORE INTO matches` statement, and the `UNIQUE(tx_hash, source_name)` constraint in `bin/tracker/migrations/001_initial.sql`.
- `protocols/*.tii` — real TII fixtures used in integration tests. Profile shapes: `jq '.profiles.mainnet' protocols/indigo.tii`.
- Existing test layout: integration tests in `bin/tracker/tests/*.rs` and `crates/tx3-lift/tests/smoke.rs`; unit tests may go in inline `#[cfg(test)]` modules.

---

### Task 1: `ProtocolAnchors` module in tx3-lift

**Files:**
- Create: `crates/tx3-lift/src/anchors.rs`
- Modify: `crates/tx3-lift/src/lib.rs` (declare module, re-export the type)
- Test: inline `#[cfg(test)]` module in `crates/tx3-lift/src/anchors.rs`

**Behavior contract** (spec §1):

`ProtocolAnchors` holds three sets — `addresses: BTreeSet<ByteBuf>`, `utxo_refs: BTreeSet<UtxoRef>`, `policies: BTreeSet<ByteBuf>` — plus three methods:

- `from_profile(&tx3_sdk::tii::spec::Profile) -> Result<Self, Error>`:
  - every `parties` value is bech32-decoded via the existing `decode_bech32_address`; decode failure is an `Err` (same error behavior as `args_from_profile`).
  - environment string values matching `<64 hex chars>#<decimal u32>` (hex case-insensitive) become `utxo_refs` entries (txid bytes + index).
  - environment string values that are exactly 56 hex chars (case-insensitive) become `policies` entries (28 raw bytes).
  - everything else is silently ignored: numbers, booleans, short/odd-length hex, 64-hex without `#` (that's 32 bytes, not a policy), `txid#notanumber`, index overflowing u32, nested objects/arrays, and a non-object `environment`.
- `is_empty()` — true iff all three sets are empty.
- `hits(&PayloadSummary) -> usize` — number of **distinct anchors** present in the tx: an address counts if it is in `input_addresses ∪ output_addresses`; a utxo ref counts if in `input_refs ∪ reference_refs`; a policy counts if in `mint_policies ∪ burn_policies ∪ value_policies`. An anchor appearing on both sides (e.g. same address in inputs and outputs) counts once.

**Steps:**

- [ ] **Step 1: Write failing unit tests** covering, at minimum:
  - parties decode into `addresses` (use a known-good bech32 from `protocols/indigo.tii`); invalid bech32 → `Err`.
  - a `"…#0"` env value lands in `utxo_refs` with correct txid bytes and index.
  - a 56-hex env value lands in `policies`; uppercase hex accepted.
  - ignored inputs: a number (`process_fee`-style), a short hex asset name, a 64-hex string with no `#`, a ref with non-numeric index, `environment` set to a non-object.
  - empty profile → `is_empty()` is true.
  - `hits()` against hand-built `PayloadSummary` values: zero on disjoint summary; counts one per distinct anchor across all three classes; an address present in both input and output sets counts once.
- [ ] **Step 2: Verify tests fail.** Run: `cargo test -p tx3-lift anchors` — expect compile error (module missing), then after stubbing, assertion failures.
- [ ] **Step 3: Implement the module** to the contract above. Reuse `decode_bech32_address`; do not duplicate hex/bech32 helpers.
- [ ] **Step 4: Verify.** Run: `cargo test -p tx3-lift` (all green, including the pre-existing smoke test), `cargo clippy -p tx3-lift --all-targets`, `cargo fmt`.
- [ ] **Step 5: Commit.** Suggested message: `feat(tx3-lift): add ProtocolAnchors derived from TII profiles`

---

### Task 2: `[matching]` config block

**Files:**
- Modify: `bin/tracker/src/config.rs`
- Test: inline `#[cfg(test)]` module in `bin/tracker/src/config.rs`

**Behavior contract** (spec §4): `Config` gains an optional `matching` section with a `mode` field; values `"all"` (default) and `"best"`. Omitting the whole block or just the field yields `All`. An unknown value fails parsing with a clear serde error. Global only — no per-source override.

**Steps:**

- [ ] **Step 1: Write failing tests**: parse a minimal TOML without `[matching]` → mode is `All`; with `mode = "best"` → `Best`; with `mode = "bogus"` → `Err`.
- [ ] **Step 2: Verify tests fail.** Run: `cargo test -p tracker config`
- [ ] **Step 3: Implement** the config types with serde defaults, mirroring the existing style of `UpstreamFilter`/`Intersect`.
- [ ] **Step 4: Verify.** Run: `cargo test -p tracker`, clippy, fmt.
- [ ] **Step 5: Commit.** Suggested message: `feat(tracker): add [matching] config with all|best mode`

---

### Task 3: schema migration + score/rank on match rows

**Files:**
- Create: `bin/tracker/migrations/002_score_rank.sql`
- Modify: `bin/tracker/src/store.rs` (`MIGRATIONS` list, `MatchRow`, `OwnedMatchRow`, the `From` impl, the INSERT statement)
- Modify: existing tests that construct match rows — `bin/tracker/tests/store_idempotency.rs`, `bin/tracker/tests/cursor_persistence.rs` (they will stop compiling once the structs gain fields; that is the failing-test signal for this task)
- Test: extend `bin/tracker/tests/store_idempotency.rs`

**Behavior contract** (spec §5): two new columns on `matches` — `score INTEGER NOT NULL DEFAULT 0` and `match_rank INTEGER NOT NULL DEFAULT 1` (named `match_rank` because `RANK` is a SQLite ≥3.25 window-function keyword). Both row structs carry the fields; the INSERT writes them. Migration appends to the existing named-migration list so it applies to both fresh databases and databases already on `001`.

**Steps:**

- [ ] **Step 1: Write/extend a failing test**: insert a row with a known score and rank, read it back via a direct SQL query, assert both values round-trip. Also assert that reopening an existing on-disk store (the `cursor_persistence.rs` pattern) applies the new migration without error.
- [ ] **Step 2: Verify failure.** Run: `cargo test -p tracker` — expect compile errors on the row structs, then test failures.
- [ ] **Step 3: Implement** the migration file and struct/INSERT changes. Update the existing tests' row constructors with explicit values.
- [ ] **Step 4: Verify.** Run: `cargo test -p tracker`, clippy, fmt.
- [ ] **Step 5: Commit.** Suggested message: `feat(tracker): persist score and match_rank on match rows`

---

### Task 4: anchors on `SpecializedTii` + zero-anchor sources disabled

**Files:**
- Modify: `bin/tracker/src/specialization.rs`
- Test: create `bin/tracker/tests/source_anchors.rs`

**Behavior contract** (spec §2): `SpecializedTii` gains a `ProtocolAnchors` field built in `specialize_one` from the already-looked-up profile. `specialize_all` excludes sources whose anchors are empty from the returned list and emits a `warn!` naming the source, the profile, and why ("no parties or recognizable environment anchors; matching disabled"). Order of surviving sources is preserved.

**Steps:**

- [ ] **Step 1: Write failing integration tests** in `source_anchors.rs`, using the real TII files via a path built from `CARGO_MANIFEST_DIR` (`../../protocols/…`):
  - configuring all five protocols with profile `mainnet` yields four active sources; `vyfi-mainnet` is dropped.
  - the indigo source's anchors contain a known party address (e.g. `cdpscript`), a known script ref (e.g. `cdp_spend_ref`'s txid#0), and a known policy id (e.g. `indy_policy_id`); expected set sizes for indigo/mainnet: 5 addresses, 8 utxo refs, 6 policies.
  - asset-name env values (e.g. indigo's `indy_name`) do NOT appear in `policies`.
- [ ] **Step 2: Verify failure.** Run: `cargo test -p tracker source_anchors`
- [ ] **Step 3: Implement** the field, construction, filtering, and warning.
- [ ] **Step 4: Verify.** Run: `cargo test -p tracker`, clippy, fmt.
- [ ] **Step 5: Commit.** Suggested message: `feat(tracker): build profile anchors per source, disable anchor-less sources`

---

### Task 5: matcher loop — gate, score, within-source dedup, cross-source rank, mode filter

**Files:**
- Modify: `bin/tracker/src/process.rs` (`run_specializations`, `apply_tx` signature), `bin/tracker/src/main.rs` (thread the matching mode from config into the call path)
- Test: inline `#[cfg(test)]` module in `bin/tracker/src/process.rs` for the selection logic

**Design guidance:** building a real `CardanoPayload` in tests is impractical, so split the work into (a) the payload-touching loop and (b) a **pure selection function** that takes the collected candidates — conceptually `(source, tx_name, score)` tuples plus whatever the lift step needs — and returns the surviving candidates with their assigned ranks. Unit-test (b) exhaustively; (a) stays thin.

**Behavior contract** (spec §3):

1. Summary computed once per tx (already the case).
2. Per source: `anchor_hits = anchors.hits(&summary)`; zero → skip the entire source before any per-tx-name work.
3. Per surviving `(source, tx_name)`: existing fingerprint pre-filter, then structural `match_tx`; candidate score = `anchor_hits + fingerprint.information_score()`.
4. Within a source, keep only the best-scoring `tx_name`; ties break alphabetically by `tx_name`.
5. Across sources, sort by score descending and assign dense 1-based ranks (equal scores share a rank).
6. Mode `Best` keeps only rank-1 rows (all of them, if tied); mode `All` keeps everything.
7. Lifting runs only for surviving candidates; `score` and `match_rank` are filled into the persisted rows.

**Steps:**

- [ ] **Step 1: Write failing unit tests** for the selection function:
  - within-source: two tx_names with different scores → higher wins; equal scores → alphabetically first wins; exactly one candidate per source survives.
  - cross-source: scores 5/5/3 → ranks 1/1/2 (dense); single candidate → rank 1.
  - mode filter: `Best` on ranks 1/1/2 keeps both rank-1 rows; `All` keeps all three.
  - empty candidate list → empty result.
- [ ] **Step 2: Verify failure.** Run: `cargo test -p tracker process`
- [ ] **Step 3: Implement**: the selection function, the gate in `run_specializations`, scoring, and threading the mode from `main.rs` through `apply_tx`. Remove nothing from the existing fingerprint/match_tx sequence other than restructuring around the gate and deferred lifting.
- [ ] **Step 4: Verify.** Run: `cargo test -p tracker` and `cargo test --workspace`, clippy, fmt.
- [ ] **Step 5: Commit.** Suggested message: `feat(tracker): gate matching on profile anchors, score and rank match rows`

---

### Task 6: over-matching regression test, docs, final verification

**Files:**
- Test: create `bin/tracker/tests/over_matching_regression.rs`
- Modify: `README.md` (pipeline description + known-debt list around line 32)

**Steps:**

- [ ] **Step 1: Write the regression test** modeled on the reported incident (`issue-over-matching.md`): build a synthetic `PayloadSummary` shaped like tx `5cfda5da…` — input/output addresses and refs taken from the issue's byte dumps (a script-like input `31c727…`, payment address `015090…` on input and outputs, no mints, no protocol refs) — load all five `protocols/*.tii` mainnet profiles, and assert `anchors.hits(&summary) == 0` for every source. This pins the "0 rows instead of 6" outcome at the gate level.
- [ ] **Step 2: Verify it passes.** Run: `cargo test -p tracker over_matching_regression` — expect PASS (the gate already exists by now; this test is regression armor, so confirm it fails if the gate is weakened, e.g. by temporarily asserting `hits > 0` to see the failure message reads well).
- [ ] **Step 3: Update README.md**: add the anchor-gate step to the pipeline description, document `score`/`match_rank` columns and the `[matching] mode` option, and append a known-limitation note that within-source `tx_name` disambiguation stays weak until the typed-flow follow-up (spec "Follow-ups" §1).
- [ ] **Step 4: Final verification.** Run: `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo fmt --check`. All green.
- [ ] **Step 5: Commit.** Suggested message: `test(tracker): regression test for over-matching incident; document anchor gate`

---

## Acceptance criteria (whole feature)

- A tx intersecting no source's anchors produces zero `matches` rows, regardless of how permissive the fingerprints are.
- `vyfi-mainnet` (current `tracker.toml`) is disabled at startup with a warning; the other four sources stay active.
- Every persisted row carries `score` and `match_rank`; at most one row per `(tx_hash, source_name)`, chosen deterministically.
- `[matching] mode = "best"` keeps only rank-1 rows; default behavior persists all ranked rows.
- `cargo test --workspace`, clippy, and `fmt --check` pass.
- No changes to `route_and_lift`, tx3-tir, or the TII files (out of scope per spec).
