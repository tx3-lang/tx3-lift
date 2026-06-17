# tx3-lift

Enrich on-chain transactions with the semantic context carried by a [Tx3](https://github.com/tx3-lang) protocol description.

A chain follower or block explorer reading raw transaction CBOR sees opaque inputs, outputs, and datums. Given the TII (Transaction Invocation Interface) document for the protocol that produced the transaction, `tx3-lift` reads the same bytes as named parties, named policies, typed datums, and labelled input/output roles.

## What it does

Three operations against a TII transaction, scoped to a profile:

1. **Fingerprint** — derive a compact set of necessary conditions (required input/output addresses, asset policies, signer hashes, slot counts, …) that any compatible payload must satisfy. Cheap to evaluate against many payloads.
2. **Match** — decide whether a payload satisfies the full TIR. Runs the fingerprint as a pre-filter, then a precise structural match.
3. **Lift** — given a matching payload, return a [`Lifted`](crates/tx3-lift/src/lift.rs) view: party annotations, input/output annotations with addresses and assets, mint/burn annotations, signers, metadata, typed datums.

The tracker additionally gates matching on **profile-derived anchors** extracted from each source's TII profile before the matcher runs: party bech32 addresses → raw address bytes, `txid#index`-shaped environment values → UTxO refs, 56-hex-char environment values → policy ids.

An anchor only *gates* (lets the structural match proceed) when its presence implies the transaction actually engaged the protocol — that the tx **ran one of its scripts** or **created stateful output at one of its addresses**. Concretely, a hit gates when it appears as: a spend from a script address (`input_addresses`), a mint or burn under an anchor policy (the minting policy executed), a reference to a deployed script-ref UTxO, or an output to a script address that **carries a datum**. Hits that anyone can produce without engaging the protocol — a bare ADA payment to a script address (no datum), or merely holding/transferring an asset the protocol issued (an anchor policy seen only in `value_policies`) — are *soft*: they raise the `score` but never gate on their own. This is what keeps, say, a third-party DEX swap that merely trades a protocol's token from matching that protocol.

Sources whose profile yields zero anchors (no parties, no qualifying environment values) are disabled at startup with a warning — under the gate they could never match anything, so the warning makes the misconfiguration loud instead of leaving a silently dead source.

The operations are exposed as standalone functions and as the `Matcher` / `Lifter` traits, so chain backends other than Cardano can plug in.

## Why profile-specialization is mandatory

The TIR inside a TII is environment-agnostic — most addresses, policies, and constants are `EvalParam` placeholders, not literals. Extracting a fingerprint from raw TIR yields almost nothing because nothing is constant.

`tx3-lift` therefore *specializes* the TIR per profile before fingerprinting: it reads `Profile.environment` (env values) plus `Profile.parties` (bech32 addresses), builds an `ArgMap`, and runs `tx3_tir::reduce::apply_args` to fold those values in. The walker then sees concrete addresses and policies. A given TII transaction yields **one fingerprint per profile** (mainnet ≠ preview ≠ preprod). True runtime parameters (e.g. `quantity` supplied at invoke time) stay unresolved and are correctly excluded from the fingerprint.

## Tracker match output

When the tracker writes a row to its `matches` table it includes two disambiguation columns:

- **`score`** — anchor hits plus the fingerprint's `information_score()` for the winning `(source, tx_name)` combination. Anchor hits count the distinct profile anchors (party addresses, script-ref UTxOs, policy ids) the tx intersects; `information_score()` adds the fingerprint's required-set entries (addresses, refs, policies, signers, metadata labels). Higher means more specific — today anchors dominate, since most fingerprints are still empty.
- **`match_rank`** — dense rank within the transaction, ordered by score descending (rank 1 = highest score; equal scores share a rank). Under the default mode every matching source produces a row, so multiple rows per tx are expected whenever more than one source matches.

The `[matching]` block in `tracker.toml` controls which candidates are retained:

```toml
[matching]
mode = "all"   # default — keep every candidate, let downstream filter by rank
# mode = "best" — keep only rank-1 rows per transaction
```

The tracker's `[upstream]` block accepts an optional `api_key` field for endpoints that require authentication. As an alternative, set the `DMTR_API_KEY` environment variable — the TOML value takes precedence when both are provided, and an empty `DMTR_API_KEY=""` is ignored.

## Crates

- [`tx3-lift`](./crates/tx3-lift) — chain-generic core: `Fingerprint`, `PayloadSummary`, `Matcher` / `Lifter` traits, `Lifted`, `specialize`.
- [`tx3-lift-cardano`](./crates/tx3-lift-cardano) — Cardano backend over [pallas](https://github.com/txpipe/pallas): `CardanoPayload` (raw CBOR + caller-supplied resolved UTxOs), `CardanoMatcher`, `CardanoLifter`, `route_and_lift` helpers.

## Status

v0, early development. APIs will change. Limitations acknowledged in v0:

- Match is a deterministic greedy algorithm; can mis-assign two TIR inputs that share the same payload UTxO and differ only by redeemer (revisit with bipartite matching when a real protocol hits this).
- Datum decoding covers `Constr`, `Map`, `Array`, `BigInt::Int`, and `BoundedBytes`. `BigInt::BigUInt`/`BigNInt` round-trip as raw bytes.
- The Cardano backend requires the caller to supply resolved input UTxOs synchronously — no `UtxoResolver` trait yet.
- Lift output's `policies` map is empty pending a TII-side policy registry.
- Within-source `tx_name` disambiguation stays weak: sibling transactions in the same TII (e.g. `open_cdp` vs. `close_cdp` in the same protocol) often produce nearly identical fingerprints because env values are substituted as opaque byte literals rather than typed constants. Score-based ranking separates *different* protocols reliably, but cannot distinguish siblings that share all the same party addresses and policies. This will improve when env values become typed constants during specialization (tracked as the "typed-flow" follow-up).
- Datum-corroborated gating only checks that an output-to-script *carries* a datum, not that the datum matches the protocol's schema. A datum-bearing output deliberately placed at a protocol's script address can still gate (it scores low and must still pass the structural match). Closing this fully needs datum-schema verification — part of the same "typed-flow" follow-up.

## License

Licensed under the [Apache License, Version 2.0](./LICENSE).
