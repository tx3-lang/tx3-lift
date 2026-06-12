# Anchor gate + scored matches — design

**Date:** 2026-06-12
**Status:** approved (design), pending implementation
**Issue:** `issue-over-matching.md` — one on-chain tx produced 6 `matches` rows
across unrelated protocols (`5cfda5da…f440`, slot `187665793`).

## Problem

The matcher is sufficient-but-not-necessary: it checks that a TIR's constraints
are present in the payload, not that the tx is unambiguously that protocol's.
Empirically (verified by dumping fingerprints for the offending pairs):

- `indigo/unstake`, `bodega/buy_position_yes`, `fluid-aquarium/create_babel_tank`,
  `vyfi/add_liquidity` all have `information_score() == 0` — their fingerprints
  are empty even though their profiles declare 4–5 parties and 10–19 environment
  constants.
- `indigo/create_cdp` (score 2) proves the party → address flow works when the
  TIR uses `to: <party>`, but environment values never become typed constants:
  policy ids are plain hex strings (no `0x` prefix) and script refs are
  `"txid#index"` strings, so `json_to_arg_value` leaves them as
  `ArgValue::String` and `const_policies_in` / `const_utxo_refs` find nothing
  after specialization.

The protocol's strongest discriminators — script-reference UTxOs, policy ids,
party addresses — sit in the profile and evaporate before reaching either the
fingerprint or the matcher.

## Decisions (settled with user)

1. **Row semantics:** persist all matching rows with `score` + `match_rank`
   columns by default; a global config option keeps only the top-ranked row.
2. **Zero-anchor sources** (e.g. `vyfi/mainnet`: `parties = {}`, environment is
   only `process_fee`): warn at startup and disable matching for that source.
   No new opt-in flag for now; future work may add metadata-based anchors.
3. **Scope:** anchor gate only. The typed-flow fix (coercing env strings into
   typed constants during specialization so fingerprints gain per-`tx_name`
   precision) is a follow-up, possibly touching tx3-tir upstream.

## Goals

- An unknown tx must not produce positive matches against unrelated protocols.
- A tx that legitimately matches several candidates is recorded with explicit
  score and rank so downstream consumers can disambiguate.
- Misconfigured (anchor-less) sources are loud at startup, not silent noise.

## Non-goals

- Per-`tx_name` disambiguation within a source beyond what fingerprints already
  give (typed-flow follow-up).
- Bipartite matching / coverage semantics (parked, see README debt note).
- Changes to `route_and_lift` in `CardanoLifter` (separate consumer; can adopt
  anchors later).

## Design

### 1. `ProtocolAnchors` — new module `crates/tx3-lift/src/anchors.rs`

Chain-neutral, derived from a TII `Profile` only (independent of the TIR
expression tree, so it works however the TIR was written — including parties
referenced only through datums).

```rust
pub struct ProtocolAnchors {
    pub addresses: BTreeSet<ByteBuf>,  // from parties (bech32-decoded, full address bytes)
    pub utxo_refs: BTreeSet<UtxoRef>,  // from env values shaped "<64 hex>#<index>"
    pub policies:  BTreeSet<ByteBuf>,  // from env values that are exactly 56 hex chars
}

impl ProtocolAnchors {
    pub fn from_profile(profile: &Profile) -> Result<Self, Error>;
    pub fn is_empty(&self) -> bool;
    /// Number of distinct anchors the summary intersects.
    pub fn hits(&self, summary: &PayloadSummary) -> usize;
}
```

Derivation rules (`from_profile`):

- Every party address is bech32-decoded via the existing
  `specialize::decode_bech32_address` → `addresses`. A decode failure is an
  error (consistent with `args_from_profile`).
- Environment string values matching `^[0-9a-fA-F]{64}#[0-9]+$` → `utxo_refs`
  (txid bytes + u32 index). Captures indigo's 8 script refs, strike's
  `strike_script_ref`, bodega's and fluid-aquarium's `*_ref` entries.
- Environment string values that are exactly 56 hex chars (28 bytes — the
  Cardano policy/script-hash length) → `policies`. A 28-byte asset name would
  be a false anchor; harmless — it simply never hits.
- Everything else (numbers, asset names, short hex, objects) is ignored.
- Non-object `environment` values yield no env anchors.

`hits` checks each anchor against the existing `PayloadSummary` sets:

| anchor set  | summary sets checked                                  |
| ----------- | ----------------------------------------------------- |
| `addresses` | `input_addresses ∪ output_addresses`                  |
| `utxo_refs` | `input_refs ∪ reference_refs`                         |
| `policies`  | `mint_policies ∪ burn_policies ∪ value_policies`      |

Address byte format is consistent end-to-end: parties decode to full address
bytes (header byte included), the same form `summarize()` and the fingerprint
use.

Known soft spot: a party that is a stake/reward address (fluid-aquarium's
`oraclescript`, `stake17…`) never appears in input/output addresses and never
hits. Acceptable — the protocol's other anchors carry it.

### 2. Startup: warn + disable zero-anchor sources

`SpecializedTii` (bin/tracker/src/specialization.rs) gains
`pub anchors: ProtocolAnchors`, built in `specialize_one` from the same
profile lookup already performed.

`specialize_all` drops sources whose anchors are empty, logging:

```
warn!(source = %src.name, profile = %src.profile,
      "profile has no parties or recognizable environment anchors; matching disabled for this source");
```

With the current `tracker.toml` this disables `vyfi-mainnet`.

### 3. Matcher loop: gate → score → rank (bin/tracker/src/process.rs)

`run_specializations` becomes:

1. **Gate.** Per source: `let anchor_hits = spec.anchors.hits(&summary);` —
   zero hits skips the whole source before any per-tx work (perf win: most txs
   touch zero sources).
2. **Candidate collection.** For each `tx_name` passing `fp.matches(&summary)`
   and `lifter.match_tx(..)`, record
   `score = anchor_hits + fp.information_score()`.
3. **Within-source dedup.** Keep only the best-scoring `tx_name` per source
   (tie-break: alphabetical `tx_name`, for determinism). This replaces today's
   arbitrary insertion-order winner under `UNIQUE(tx_hash, source_name)`.
   Caveat (accepted): within-source disambiguation stays weak until the
   typed-flow follow-up gives sibling fingerprints distinct content.
4. **Cross-source rank.** Sort candidates by score descending; assign dense
   1-based `match_rank` (equal scores share a rank).
5. **Mode filter.** If `matching.mode == "best"`, keep only rank-1 rows
   (including ties — residual ambiguity stays visible). Default `"all"` keeps
   every row.
6. **Lift.** Lifting runs after filtering so dropped candidates cost nothing.

Scoring rationale: both terms count matched facts. `anchor_hits` dominates
today (fingerprints are mostly empty); `information_score()` refines ranking
within and across sources as fingerprints get richer.

Expected effect on the reported reproduction: tx `5cfda5da…` intersects no
anchor of any of the 5 configured protocols → **0 rows** instead of 6.

### 4. Config (bin/tracker/src/config.rs)

```toml
[matching]          # optional block
mode = "all"        # "all" (default) | "best"
```

Global only — rank is cross-source, so a per-source override would be
meaningless.

```rust
#[derive(Default)]
pub struct MatchingConfig { pub mode: MatchMode }
pub enum MatchMode { #[default] All, Best }
```

### 5. Schema (bin/tracker/migrations/002_score_rank.sql)

```sql
ALTER TABLE matches ADD COLUMN score      INTEGER NOT NULL DEFAULT 0;
ALTER TABLE matches ADD COLUMN match_rank INTEGER NOT NULL DEFAULT 1;
```

Column is `match_rank` (not `rank`) because `RANK` is a window-function keyword
in SQLite ≥ 3.25. `MatchRow` / `OwnedMatchRow` and the INSERT statement gain
both fields. The migration is appended to the existing `MIGRATIONS` list.

## Testing

Unit (`crates/tx3-lift`, anchors module):

- parties decode into `addresses`; bad bech32 → error
- `"txid#0"` env strings parse into `utxo_refs`; malformed refs ignored
- 56-hex env strings land in `policies`; numbers (`process_fee`), short hex,
  asset names ignored
- empty profile → `is_empty()`
- `hits()` against synthetic `PayloadSummary`: counts distinct intersections
  across all three anchor classes; zero when disjoint

Tracker (bin/tracker):

- zero-anchor source is dropped by `specialize_all` (and warned)
- gate: a payload intersecting no anchors produces zero rows even when
  fingerprints trivially match
- score and `match_rank` populate correctly for multi-source hits;
  dense-rank ties share a rank
- `mode = "best"` keeps only rank-1 rows; default keeps all
- within-source: single row per `(tx, source)`, best score wins, tie-break
  alphabetical
- where practical, a fixture test using the real `protocols/*.tii` mainnet
  profiles reproducing the issue scenario (anchors disjoint from the
  `5cfda5da…` tx shape → 0 rows)

## Follow-ups (out of scope, tracked for later)

1. **Typed-flow fix:** coerce env strings (hex → bytes, `txid#index` →
   UtxoRef) during specialization so fingerprints/matcher gain per-`tx_name`
   precision; may need tx3-tir changes.
2. **Anchor sources for party-less protocols:** metadata labels or explicit
   anchor declarations in the TII for cases like vyfi.
3. **`route_and_lift` adoption:** apply the same gate in the library-level
   routing path.
