# tx3-lift

Enrich on-chain transactions with the semantic context carried by a [Tx3](https://github.com/tx3-lang) protocol description.

A chain follower or block explorer reading raw transaction CBOR sees opaque inputs, outputs, and datums. Given the TII (Transaction Invocation Interface) document for the protocol that produced the transaction, `tx3-lift` reads the same bytes as named parties, named policies, typed datums, and labelled input/output roles.

## What it does

Three operations against a TII transaction, scoped to a profile:

1. **Fingerprint** — derive a compact set of necessary conditions (required input/output addresses, asset policies, signer hashes, slot counts, …) that any compatible payload must satisfy. Cheap to evaluate against many payloads.
2. **Match** — decide whether a payload satisfies the full TIR. Runs the fingerprint as a pre-filter, then a precise structural match.
3. **Lift** — given a matching payload, return a [`Lifted`](crates/tx3-lift/src/lift.rs) view: party annotations, input/output annotations with addresses and assets, mint/burn annotations, signers, metadata, typed datums.

The operations are exposed as standalone functions and as the `Matcher` / `Lifter` traits, so chain backends other than Cardano can plug in.

## Why profile-specialization is mandatory

The TIR inside a TII is environment-agnostic — most addresses, policies, and constants are `EvalParam` placeholders, not literals. Extracting a fingerprint from raw TIR yields almost nothing because nothing is constant.

`tx3-lift` therefore *specializes* the TIR per profile before fingerprinting: it reads `Profile.environment` (env values) plus `Profile.parties` (bech32 addresses), builds an `ArgMap`, and runs `tx3_tir::reduce::apply_args` to fold those values in. The walker then sees concrete addresses and policies. A given TII transaction yields **one fingerprint per profile** (mainnet ≠ preview ≠ preprod). True runtime parameters (e.g. `quantity` supplied at invoke time) stay unresolved and are correctly excluded from the fingerprint.

## Crates

- [`tx3-lift`](./crates/tx3-lift) — chain-generic core: `Fingerprint`, `PayloadSummary`, `Matcher` / `Lifter` traits, `Lifted`, `specialize`.
- [`tx3-lift-cardano`](./crates/tx3-lift-cardano) — Cardano backend over [pallas](https://github.com/txpipe/pallas): `CardanoPayload` (raw CBOR + caller-supplied resolved UTxOs), `CardanoMatcher`, `CardanoLifter`, `route_and_lift` helpers.

## Status

v0, early development. APIs will change. Limitations acknowledged in v0:

- Match is a deterministic greedy algorithm; can mis-assign two TIR inputs that share the same payload UTxO and differ only by redeemer (revisit with bipartite matching when a real protocol hits this).
- Datum decoding covers `Constr`, `Map`, `Array`, `BigInt::Int`, and `BoundedBytes`. `BigInt::BigUInt`/`BigNInt` round-trip as raw bytes.
- The Cardano backend requires the caller to supply resolved input UTxOs synchronously — no `UtxoResolver` trait yet.
- Lift output's `policies` map is empty pending a TII-side policy registry.

## License

Licensed under the [Apache License, Version 2.0](./LICENSE).
