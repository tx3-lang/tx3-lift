# Anchor Strength Gating Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop value-policy-only and bare-output-to-script anchor hits from gating a match, so a tx only matches a protocol when it runs one of its scripts or creates a stateful (datum-bearing) output at its address.

**Architecture:** Classify each anchor hit by whether it implies a script execution / stateful interaction. `summarize()` records which output addresses carry a datum; `ProtocolAnchors::hits` returns a tiered `AnchorHits { gating, total }`; the tracker gates on `gating > 0` and scores on `total`. The persisted `score` formula and the score/rank schema are unchanged.

**Tech Stack:** Rust workspace; `tx3-lift` (chain-neutral core), `tx3-lift-cardano` (pallas), `tracker` binary.

**Spec:** `docs/superpowers/specs/2026-06-16-anchor-strength-gating-design.md` — read it fully before starting. This plan contains **NO implementation code** (per the user's standing preference): each task gives behavior contracts, acceptance criteria, exact file pointers, and verification commands; the implementer writes the code. TDD ordering still applies — write the failing tests from the acceptance criteria first, watch them fail, then implement.

**Conventions:** conventional commits (`feat:`/`test:`/`refactor:`/`docs:`); run `cargo fmt` and `cargo clippy --workspace --all-targets` before each commit; **every commit must leave `cargo test --workspace` green** (no commit may leave the workspace non-compiling). There are ~14 pre-existing clippy warnings (mostly `result_large_err`); introduce no new ones, fix none of the old ones.

---

## Context for the implementer (read first, ~10 min)

- `docs/superpowers/specs/2026-06-16-anchor-strength-gating-design.md` — the design; §1 has the tier table, which is the spec of `hits()`.
- `crates/tx3-lift/src/payload.rs` — `PayloadSummary` (all fields `pub`, `BTreeSet<ByteBuf>` for address/policy sets, `UtxoRef = (ByteBuf, u32)`). Derives `Default`.
- `crates/tx3-lift-cardano/src/summarize.rs` — builds `PayloadSummary` from a parsed pallas tx. The output loop (around line 32) already calls `output.address()`; this is where datum detection goes. `pallas::ledger::traverse::MultiEraOutput::datum()` returns `Option<DatumOption>` (verified).
- `crates/tx3-lift/src/anchors.rs` — `ProtocolAnchors` with `addresses`/`utxo_refs`/`policies` sets and `hits()` (line 86, currently `-> usize`). Its `#[cfg(test)]` module (from line ~386) has one test per anchor class — these are the migration scaffold for Task 2.
- `bin/tracker/src/process.rs:156` — the gate (`spec.anchors.hits(&summary)`), and the score at the candidate-collection site (`score = anchor_hits + fp.information_score()`).
- `bin/tracker/tests/over_matching_regression.rs:114,148` — the only other `hits()` callers (negative + positive control).
- Provided fixtures — real mainnet tx CBOR, hex, no `0x` prefix, already on disk (committed per task). Sourced from `/Users/mduthey/Documents/Work/txpipe/api-layer-tx3-protocols/protocols_tx3/B2-successful-api-calls.md` (genuine tx3-built submissions) and the live-run incident. Each was decoded with pallas to produce the ground-truth below.

  **`crates/tx3-lift-cardano/tests/fixtures/sp_deposit_71e89010.cbor.hex`** (~1200 bytes) — for the Task 1 summarize test:
  - 4 outputs. `output_addresses` has **3** distinct addresses; `output_addresses_with_datum` has **2**.
  - WITH datum: `311c53ed6f616687b340ac83072ec65a9787583c01d6bae0314e1d61d0b8358aadd30c60eba168608ad5e875592e9b7cb8c700827cde87f9a3` (script addr, in two outputs) and `011ff8ec747a4655f4f3abfe66233fb0343954025143e9134ca779640deb28ab85fa3384a04ef6778e5816fdb3412c6c2a9956cedd011f0dbe`.
  - WITHOUT datum: `01d31ae59bac6318cbf598a2b417ebdb5092f16b31472856fffff5e4777aa4bc5d02917c227014f9ed0d16cf096b0aa8fdc4aa3ddb374f98ce` (change output) — must be absent from the datum set.
  - Addresses in `PayloadSummary` are raw bytes (`addr.to_vec()`), so assert on `ByteBuf::from(hex::decode(<above>))`.

  **`bin/tracker/tests/fixtures/indigo_create_staking_c54778b4.cbor.hex`** (~960 bytes) — Task 3 positive control. Against indigo/mainnet anchors, decoded: gating via output-with-datum at a staking script address (1 hit), a control-NFT mint (1 hit), AND two script-ref references (2 hits) → **`gates() == true`**. (Also carries 3 indigo policies as value, which are soft.)

  **`bin/tracker/tests/fixtures/dex_swap_iusd_06a73a03.cbor.hex`** (~1200 bytes) — Task 3 negative control; the exact false positive from the live run. Against indigo/mainnet anchors: zero gating hits (no script-input, no mint, no script-ref, no datum-output at an indigo address); the only intersection is the iAsset value-policy `f66d78b4a3cb3d37afa0ec36461e51ecbde00f26c8f0a68f94b69880` → soft → **`gates() == false`, `total == 1`**.

---

### Task 1: `output_addresses_with_datum` on PayloadSummary + populate in summarize

**Files:**
- Modify: `crates/tx3-lift/src/payload.rs` (add field)
- Modify: `crates/tx3-lift-cardano/src/summarize.rs` (populate field)
- Add fixture (already on disk, just `git add`): `crates/tx3-lift-cardano/tests/fixtures/sp_deposit_71e89010.cbor.hex`
- Test: create `crates/tx3-lift-cardano/tests/summarize_datum.rs` (new integration test; the crate has no test dir yet)

**Behavior contract:**
- `PayloadSummary` gains `pub output_addresses_with_datum: BTreeSet<ByteBuf>` — the addresses of outputs that carry a datum. It is a subset of `output_addresses`. `Default` must still derive (so all existing `PayloadSummary::default()` call sites keep compiling).
- In `summarize()`, within the existing `for output in tx.outputs()` loop, when the output's address decodes AND `output.datum().is_some()`, insert the address bytes into `output_addresses_with_datum` (in addition to the unconditional insert into `output_addresses`). No other summary field changes. Inputs are not considered (this field is output-only).

**Acceptance criteria / tests** (`summarize_datum.rs`, loading the fixture via `include_str!` + `hex::decode`, building a `CardanoPayload` from the bytes, calling the crate's summarize entry point):
- `output_addresses_with_datum` has exactly 2 entries.
- It contains the script address `311c53ed6f…a3` (hex above) and the payment address `011ff8ec…be`.
- It does NOT contain the change address `01d31ae5…ce`.
- It is a strict subset of `output_addresses` (which has 3 entries).

**Steps:**
- [ ] **Step 1: Write the failing test** `summarize_datum.rs` with the four assertions above. Build the payload with `tx3_lift_cardano::payload::CardanoPayload::from_cbor(hex::decode(include_str!("fixtures/sp_deposit_71e89010.cbor.hex").trim())?)` and summarize with `tx3_lift_cardano::summarize::summarize(&payload)` (both `pub`, confirmed). No resolved inputs needed — the field is output-only.
- [ ] **Step 2: Run it, verify it fails to compile** (`cargo test -p tx3-lift-cardano --test summarize_datum`) — the field doesn't exist yet.
- [ ] **Step 3: Add the field** to `PayloadSummary`, then **populate it** in `summarize()`.
- [ ] **Step 4: Verify** `cargo test -p tx3-lift-cardano` green; `cargo test --workspace` still green (additive field, no caller breaks); clippy clean; `cargo fmt`.
- [ ] **Step 5: Commit** (include the fixture): `feat(tx3-lift): record output addresses carrying a datum in PayloadSummary`

---

### Task 2: tiered `AnchorHits` + tracker gate/score wiring

**Files:**
- Modify: `crates/tx3-lift/src/anchors.rs` (`hits()` return type + tier logic; migrate the `#[cfg(test)]` assertions; add datum/soft tests)
- Modify: `bin/tracker/src/process.rs` (gate on `.gates()`, score on `.total`)
- Modify: `bin/tracker/tests/over_matching_regression.rs` (new return type + stronger control)

This is one atomic task because changing `hits()`'s signature breaks `process.rs` and the regression test; they must move together so every commit stays compilable.

**Behavior contract:**
- New public type in `anchors.rs`: `AnchorHits { pub gating: usize, pub total: usize }` with `pub fn gates(&self) -> bool` returning `self.gating > 0`. Re-export it from `tx3-lift`'s `lib.rs` alongside `ProtocolAnchors` if that crate re-exports public types (match the existing export style).
- `ProtocolAnchors::hits(&self, &PayloadSummary) -> AnchorHits`, classifying per the spec §1 tier table:
  - **gating** presence for an anchor: address ∈ `input_addresses`; OR address ∈ `output_addresses_with_datum`; OR policy ∈ `mint_policies ∪ burn_policies`; OR utxo_ref ∈ `input_refs ∪ reference_refs`.
  - **soft** presence: address ∈ `output_addresses` but NOT in `output_addresses_with_datum`; OR policy ∈ `value_policies` only.
  - `total` = count of distinct anchors with ANY presence (gating or soft). This MUST equal the old flat `hits()` value for any given summary (so `score` is unchanged).
  - `gating` = count of distinct anchors with AT LEAST ONE gating presence. Each anchor counts at most once in each of `gating`/`total`.
- `process.rs`: gate becomes "skip the source when `!hits.gates()`" (still before the per-tx_name loop). Score becomes `hits.total + fp.information_score()` (same numeric result as before for strong matches). Nothing else in the loop changes.

**Acceptance criteria / tests:**

*Anchors unit tests (migrate the existing per-class tests + add new ones).* For a single-anchor profile and a synthetic summary placing that anchor in one class, assert both `.gating` and `.total`:
- spend-from-script (anchor address in `input_addresses`) → `gating == 1, total == 1`
- mint policy / burn policy (anchor policy in `mint_policies` / `burn_policies`) → `gating == 1, total == 1`
- script-ref in `input_refs` / in `reference_refs` → `gating == 1, total == 1`
- output-to-script WITH datum (anchor address in BOTH `output_addresses` AND `output_addresses_with_datum`) → `gating == 1, total == 1` **(new test)**
- output-to-script WITHOUT datum (anchor address in `output_addresses` only) → `gating == 0, total == 1`  *(this is the migrated `hits_counts_address_in_outputs`, semantics changed)*
- value-policy only (anchor policy in `value_policies` only) → `gating == 0, total == 1`  *(migrated `hits_counts_policy_in_value_policies`)*
- disjoint summary → `gating == 0, total == 0`
- mixed: an address present as both a spend (`input_addresses`) and a bare output (`output_addresses`, no datum) → counts once → `gating == 1, total == 1` *(migrated `address_in_both_input_and_output_counts_once`)*
- a multi-anchor case combining one gating and one soft anchor → e.g. `gating == 1, total == 2` **(new test, pins that soft anchors raise total but not gating)**
- `gates()` is true iff `gating > 0`

*Regression test (`over_matching_regression.rs`):*
- Update both `hits()` call sites to the new return type. The incident summary must yield `total == 0` and `gates() == false` (it intersects no anchor).
- Strengthen the positive control: today it puts the Indigo `cdpscript` address in `output_addresses` (which is now SOFT → would not gate). Change it so the indigo anchor lands in a GATING position — put the address in `input_addresses` (or in BOTH `output_addresses` and `output_addresses_with_datum`) — and assert `gates() == true`.
- Add a companion assertion (same or sibling test): a summary where the indigo `cdpscript` address is in `output_addresses` only (no datum) AND/OR the iAsset policy is in `value_policies` only → `gates() == false`, `total >= 1`. This pins the exact false-positive class from the live run (the DEX swap that only carried iUSD).

**Steps:**
- [ ] **Step 1: Write/adjust the failing tests** — migrate every existing `assert_eq!(anchors.hits(&s), N)` in `anchors.rs` to assert `.gating`/`.total` per the table above, add the new datum-gating, soft-vs-gating, and mixed tests, and update the regression test's two call sites + controls. They won't compile yet (type change).
- [ ] **Step 2: Run, verify failure** — `cargo test -p tx3-lift anchors` (compile error: `AnchorHits` missing), then assertion failures after the type is stubbed.
- [ ] **Step 3: Implement** `AnchorHits` + the tiered `hits()` logic; update `process.rs` (gate `.gates()`, score `.total`).
- [ ] **Step 4: Verify** `cargo test --workspace` green; `cargo clippy --workspace --all-targets` no new warnings; `cargo fmt --check` clean.
- [ ] **Step 5: Commit** `feat: gate matching on script-execution anchors, demote value/ bare-output hits to score-only`

---

### Task 3: real-tx end-to-end gating test

**Files:**
- Add fixtures (already on disk, `git add`): `bin/tracker/tests/fixtures/indigo_create_staking_c54778b4.cbor.hex`, `bin/tracker/tests/fixtures/dex_swap_iusd_06a73a03.cbor.hex`
- Test: create `bin/tracker/tests/gating_real_txs.rs`

**Why:** The Task 2 tests are synthetic (hand-built summaries). This task proves the full stack — real tx CBOR → `summarize()` → `ProtocolAnchors::hits()` → `gates()` — on the actual transactions that motivated the change, using the real `protocols/indigo.tii` mainnet profile.

**Behavior contract / acceptance criteria** (`gating_real_txs.rs`, reusing the `#[path]`-include + `protocol_path()` helpers from `bin/tracker/tests/source_anchors.rs`; load indigo via `specialize_all` to get its `ProtocolAnchors`, build a `CardanoPayload` from each fixture, `summarize()` it, then call `anchors.hits(&summary)`):
- **Negative — DEX swap (`dex_swap_iusd_06a73a03`):** `hits.gates() == false` AND `hits.total == 1`. This is the live-run false positive; its lone anchor intersection is the iAsset value-policy, now soft. (Note: `summarize()` runs with empty `resolved_inputs` here — the iAsset policy is still captured from the output value, so `total == 1` holds without resolved inputs.)
- **Positive — indigo `create_staking` (`indigo_create_staking_c54778b4`):** `hits.gates() == true`. Optionally assert `hits.gating >= 1` (it has multiple gating signals: a datum-bearing output at a staking script address, a control-NFT mint, and script-ref references).

**Steps:**
- [ ] **Step 1: Write the failing test** with the two cases above. (It will fail to compile until Tasks 1–2 land — this task runs after them.)
- [ ] **Step 2: Run, verify it passes** — `cargo test -p tracker --test gating_real_txs`. Confirm the negative asserts `gates()==false`/`total==1` and the positive asserts `gates()==true`. Sanity-check it would fail if gating were weakened (e.g. temporarily assert the DEX swap gates, see the failure, revert — do not commit).
- [ ] **Step 3: Verify** `cargo test --workspace` green; clippy no new warnings; `cargo fmt --check`.
- [ ] **Step 4: Commit** (include both fixtures): `test(tracker): real-tx end-to-end gating (DEX-swap dropped, indigo interaction kept)`

---

### Task 4: README + final verification

**Files:**
- Modify: `README.md` (the anchor-gate paragraph + the known-limitations area)

**Steps:**
- [ ] **Step 1: Update README.md.** In the anchor-gate paragraph, add that a hit only *gates* when it implies a script ran (spend from a script address, mint/burn under an anchor policy, use of a deployed script-ref) or creates a stateful output (an output to a script address that carries a datum); merely holding a protocol-issued asset or paying bare ADA to a script address contributes to `score` but does not gate. Keep the existing voice; do not restate the whole pipeline.
- [ ] **Step 2: Note the residual limitation** near the existing known-limitations bullet: a deliberately datum-bearing output at a protocol address can still gate (closing it fully needs datum-schema matching — the typed-flow follow-up).
- [ ] **Step 3: Final verification** — run `cargo test --workspace`, `cargo clippy --workspace --all-targets` (no new warnings), `cargo fmt --check` (clean). Paste the test summary lines.
- [ ] **Step 4: Commit** `docs: document script-execution gating and its residual limitation`

---

## Acceptance criteria (whole feature)

- A summary whose only anchor presence is a value-policy or a bare (no-datum) output-to-script yields `gates() == false` → no match row.
- A summary with a spend-from-script, a mint/burn under an anchor policy, a script-ref, or a datum-bearing output-to-script yields `gates() == true`.
- `total` (hence persisted `score`) is unchanged from the previous flat `hits()` count for every summary; the score/rank schema and `[matching]` config are untouched.
- `summarize()` populates `output_addresses_with_datum` correctly against the real fixture.
- End-to-end on real mainnet txs against the real `indigo.tii`: the DEX swap does not gate (`total == 1`), a genuine indigo `create_staking` does gate.
- `cargo test --workspace`, clippy (no new warnings), and `cargo fmt --check` all pass; every commit leaves the workspace green.

## Out of scope (tracked, not in this plan)

- Updating the stale Indigo Stability Pool validator address in `protocols/indigo.tii` (`88e02990…` → `1c53ed6f…`) — a data task; with this change a genuine SP tx will not match until the TII is corrected (expected, accepted by the user).
- Datum-schema / redeemer verification (option 3) and per-`tx_name` disambiguation (typed-flow follow-up).
