# Anchor strength gating (hard/soft + datum corroboration) — design

**Date:** 2026-06-16
**Status:** approved (design), pending implementation
**Builds on:** `docs/superpowers/specs/2026-06-12-anchor-gate-matching-design.md`

## Problem

The anchor gate (shipped in the prior increment) lets a transaction match a
protocol as soon as it intersects **one** of the profile's anchors, treating
all anchor classes as equally strong. A live run against mainnet exposed the
gap: of 16 match rows, 14 were `score == 1` (a single anchor hit), and that
single hit was almost always the **value-policy of a fungible asset the
protocol issues** (Indigo's `iasset_policy_id`, the iUSD/iBTC policy).

Two concrete transactions:

- `06a73a03…0788f` — a third-party DEX stableswap that merely *trades* iUSD.
  It touches no Indigo validator, yet matched `indigo/unstake` because iUSD's
  policy is an Indigo anchor. **False positive.**
- `71e890106c10…dc3c` — a genuine Indigo Stability Pool deposit. It only
  scored 1 (the iUSD policy again) because the profile's Stability Pool
  validator address (`88e02990…`) is stale — the live validator is
  `1c53ed6f…`. So even a real interaction scrapes by on the weak anchor, and
  gets mislabeled `unstake`. (The mislabeling is the separate, already-tracked
  typed-flow follow-up; the *stale profile data* is a TII-maintenance issue,
  not a code defect.)

Root cause: **an anchor's discriminative power depends on whether a script was
forced to execute, not on which class it belongs to.** A minted-asset policy,
or a bare output paid to a script address, are things anyone can produce
without interacting with the protocol. A spend from a script address, or a
mint/burn under the protocol's policy, force a script to run — you cannot do
them unless the protocol's logic permits.

## Decision (settled with user)

Classify every anchor hit by whether it implies a script execution (or stateful
interaction), and **gate only on the strong tiers**. Corroborate the one
ambiguous case — paying to a script address — with **datum presence** on that
output (the "2-light" variant: datum present, not datum-schema-matched).

Rejected alternatives:
- *Require ≥1 address-or-ref hit* (the first proposal): output-to-script is
  spoofable, so an address hit isn't inherently strong.
- *Classify policies as NFT-vs-fungible by env-key name*: fragile heuristic.
  Superseded — we classify by **where the hit lands** (minted in this tx vs
  merely present as value), which is observable per-tx and needs no naming
  convention.
- *Datum-schema verification* (the "2-precise"/option-3 path): decode the
  output datum against the TIR's expected schema. Bigger, overlaps the
  typed-flow follow-up. Out of scope here.

## Goals

- A tx that only *holds/trades* a protocol-issued asset, or only pays bare ADA
  to a protocol script address, must **not** match.
- A tx that runs one of the protocol's scripts (spend-from-script, mint/burn
  under an anchor policy), uses its deployed script (script-ref), or creates a
  stateful output at its address (datum present) **does** match.
- The persisted `score` keeps its current meaning (total distinct anchors hit +
  `information_score()`); only the **gate decision** changes.

## Non-goals

- Datum-schema matching / redeemer verification (option 3, future).
- Fixing stale TII profile data (a data task, separate from code).
- Per-`tx_name` disambiguation within a source (the typed-flow follow-up).
- Touching `route_and_lift`, tx3-tir, or the score/rank schema (migration 002
  stays as-is).

## Design

### 1. Anchor hit tiers

For each distinct anchor, classify its presence in the tx:

| anchor matches in…                              | meaning                         | tier        |
| ----------------------------------------------- | ------------------------------- | ----------- |
| `input_addresses` (spend from script address)   | the validator executed          | **gating**  |
| `mint_policies ∪ burn_policies`                 | the minting policy executed     | **gating**  |
| `input_refs ∪ reference_refs` (script-ref UTxO)  | the deployed script is in use   | **gating**  |
| `output_addresses` ∩ `output_addresses_with_datum` | stateful output created      | **gating**  |
| `output_addresses` only (no datum)              | bare payment to a script        | soft        |
| `value_policies` (asset merely present)         | the asset is circulating        | soft        |

**Gate rule:** a source matches a tx iff it has **≥1 gating-tier hit**. Soft
hits contribute to `score` but never gate on their own.

Counting preserves "distinct anchors" semantics: for each anchor, it counts
once. An anchor with *any* gating presence is a gating hit; an anchor with any
presence at all is part of the total. (E.g. an address present as both a spend
and a bare output is one gating hit.)

### 2. `PayloadSummary` gains one field

```rust
pub output_addresses_with_datum: BTreeSet<ByteBuf>,
```

A subset of `output_addresses`: addresses of outputs that carry a datum.
`Default` still derives. No other summary field changes.

`summarize()` (tx3-lift-cardano) already loops `tx.outputs()` calling
`output.address()`; add, in the same loop, `if output.datum().is_some()` →
insert the address into the new set. (`MultiEraOutput::datum()` returns
`Option<DatumOption>` — verified.) ~5 lines, no extra traversal.

### 3. `ProtocolAnchors::hits` returns structured tiers

Change the return type from `usize` to:

```rust
pub struct AnchorHits {
    pub gating: usize,  // distinct anchors with a gating-tier presence
    pub total:  usize,  // distinct anchors present at all (gating + soft)
}
impl AnchorHits {
    pub fn gates(&self) -> bool { self.gating > 0 }
}
```

`total` equals the old flat `hits()` value, so `score` is unchanged. `gating`
is the new gate signal. The function reads the new `output_addresses_with_datum`
set to decide whether an output-address hit is gating or soft.

### 4. Tracker wiring (process.rs)

- Gate: replace `spec.anchors.hits(&summary) == 0 → skip` with
  `!spec.anchors.hits(&summary).gates() → skip` (still before any per-tx_name
  work).
- Score: `score = hits.total + fp.information_score()` — same formula, same
  persisted values for genuinely-strong matches.

No change to scoring/rank/mode selection, the schema, or `[matching]` config.

### 5. Startup zero-anchor disabling is unchanged

`is_empty()` (no anchors at all) still disables a source at startup. A source
whose anchors exist but never gate is a per-tx outcome, not a startup decision.

## Expected effect on the observed data

- `06a73a03…` (DEX swap): only soft (value-policy iUSD; no spend-from-script,
  no mint, no datum output at an Indigo address) → `gating == 0` → **dropped**.
- `create_cdp`-style txs: mint the CDP control NFT → gating via mint → **kept**
  (don't even rely on the output).
- request/staging legs that pay to a script address **with a datum** → gating
  via datum corroboration → **kept**.
- `71e890106c…` (real SP deposit, stale profile address): with the stale
  address it has no gating hit → **dropped** — correct *given wrong data*; fix
  the TII address and it gates via spend-from-script.

The 14 `score == 1` value-policy rows collapse to zero; the `score == 4`
bodega rows (multiple anchors incl. strong ones) are unaffected.

## Residual risk (accepted)

A datum-bearing output deliberately constructed at a protocol's script address
would gate even without an NFT or a real interaction. This is far more effort
than an incidental token transfer, scores low, and still must pass the
structural matcher. Closing it fully is option 3 (datum-schema match). If
tighter gating is wanted later, the corroboration can be upgraded to "datum
**and** an anchor-policy NFT on the same output" by changing the summary field
from a set to a `BTreeMap<address, BTreeSet<policy>>` — same shape of change.

## Testing

Unit (`tx3-lift`, anchors): build synthetic `PayloadSummary` values and assert
tier classification —
- spend-from-script (input_addresses) → gating
- mint/burn under anchor policy → gating
- script-ref in reference_refs / input_refs → gating
- output-to-script **with** datum → gating
- output-to-script **without** datum → soft (total but not gating)
- value-policy only → soft
- mixed: one anchor present as both spend and bare output counts once as gating
- `gates()` true iff ≥1 gating hit; `total` matches the old flat count

Unit (`tx3-lift-cardano`, summarize): a CBOR fixture with one datum-bearing
output and one plain output → `output_addresses_with_datum` contains only the
former. (Reuse/extend an existing fixture if available; otherwise build one.)

Tracker / regression:
- Update `over_matching_regression.rs` to the new return type. The incident
  summary stays `total == 0` and `gates() == false`. Strengthen the positive
  control so it tests the *gating* tiers (put the Indigo address in
  `input_addresses`, or in `output_addresses_with_datum`) rather than a bare
  output — and add a companion assertion that a bare output / value-policy-only
  summary does **not** gate.
- Update any other callers of `hits()` for the new return type.

## Relevant files

- `crates/tx3-lift/src/payload.rs` — `PayloadSummary` (+1 field)
- `crates/tx3-lift-cardano/src/summarize.rs` — populate the new field
- `crates/tx3-lift/src/anchors.rs` — `hits()` → `AnchorHits`, tier logic
- `bin/tracker/src/process.rs` — gate on `.gates()`, score on `.total`
- `bin/tracker/tests/over_matching_regression.rs` — return-type + stronger control
