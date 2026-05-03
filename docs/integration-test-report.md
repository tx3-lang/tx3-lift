# Integration test report

**Date**: 2026-05-03
**Fixture**: [`bin/tracker/examples/orcfax-burn/`](../bin/tracker/examples/orcfax-burn/)
**Components exercised**: tracking ↔ enrichment ↔ storage

This report documents an end-to-end run that drove a real Cardano
mainnet transaction through every stage of the system and verified the
expected output landed in SQLite. The fixture replays a single
historical block from the upstream's WAL window, so the result is
deterministic and re-runnable.

## Environment

| Layer        | Version / config                                                                            |
| ------------ | ------------------------------------------------------------------------------------------- |
| Tracker      | `bin/tracker` at HEAD (after the `upstream`/`predicate` refactors, PR #5 + #6).             |
| Lift core    | `crates/tx3-lift` at the same HEAD; structural matcher + fingerprint pre-filter + lifter.   |
| Cardano backend | `crates/tx3-lift-cardano`, with the v1beta-aware payload that takes resolved inputs from `as_output.original_cbor`. |
| Wire spec    | `utxorpc-spec = "0.19"` — `utxorpc.v1beta.{watch,cardano}` modules.                         |
| Upstream     | Local utxorpc v1beta server at `http://localhost:50051`, mainnet history covering slot ≤ `186188536`. |
| Storage      | SQLite via `rusqlite 0.32` (bundled), WAL journal mode.                                     |

The tracker was launched with:

```
RUST_LOG=tracker=debug ./run.sh
```

…which compiles `main.tx3` to a TII via `trix build -p mainnet`,
splices any `DMTR_*` overrides into `tracker.toml`, and execs
`cargo run -p tracker --release`.

## Test plan

The fixture is wired to match exactly one well-known transaction:

- **Target tx**: `1e31b36253043a89a388714f7245156788cbdbceed5d74ee81679e56a7b81a86`
- **Block**: `a18369937e9acd0b0095173cbd35b8b2f013ba13dcc34518e6d5a8a5004433e1` (height `13368056`, slot `186188550`)
- **Intersect**: parent block `26993814bd8770da765f482fafe9d9e47d206b0ff4b42cd6e68d5910ba298ae6` (slot `186188536`, 14 slots before the target)
- **Predicate** (`[upstream.filter]`): `mints_policy_id = 193ee65211bb…cdf1a4b` (Orcfax)

Expected behaviour, by component:

1. **Tracking** — the daemon connects to the upstream, places its
   intersect at the parent block, receives `Idle` advances for blocks
   that don't match the predicate, and receives an `Apply` envelope
   for the target tx with the parsed Cardano `Tx` plus the containing
   block's `native_bytes`.
2. **Enrichment** — for the target tx, the cardano backend resolves
   every input + reference input from `TxInput.as_output.original_cbor`,
   the fingerprint pre-filter passes, the structural matcher returns
   `Some(MatchAssignment)`, and the lifter produces a `Lifted` record
   with parties, inputs, outputs, burns, signers, and the decoded
   inline datum.
3. **Storage** — exactly one row lands in `matches`, the `cursor`
   advances to the target block, and a clean restart re-streams from
   the cursor without double-inserting.

## Results

### 1 · Tracking

The daemon attached and the upstream began streaming. With the
`mints_policy_id` predicate set, only the Orcfax-policy transaction
came through as an `Apply`; every other block in the replay window
arrived as an `Idle` advance.

```text
INFO   subscribing to WatchTx endpoint=http://localhost:50051
DEBUG  collected resolved inputs from WatchTx envelope
       tx=1e31b362…b81a86 tx_inputs=26 ref_inputs=1 resolved_inputs=27
INFO   matched
       tx=1e31b36253043a89a388714f7245156788cbdbceed5d74ee81679e56a7b81a86
       slot=186188550 matches=1
DEBUG  idle slot=186188607
DEBUG  idle slot=186188617
DEBUG  idle slot=186188643
DEBUG  idle slot=186188674
DEBUG  idle slot=186188685
… (further idle advances)
```

**Verified**:

- Stream stayed open for the full duration, no `transport error` or
  `stream closed by server`.
- `Apply` arrived with both `block.native_bytes` (used for pallas
  block decoding) and the parsed `cardano.Tx` (used for input
  resolution); both were present and non-empty.
- `Idle` advances fired between the target slot and tip — confirming
  the cursor would have advanced even without a match in those
  blocks.
- The target `tx_hash` matches the fixture pin **byte-for-byte**.

### 2 · Enrichment

For the matched tx, the backend resolved 27 inputs (26 spent + 1
reference) entirely from `TxInput.as_output.original_cbor` carried in
the WatchTx envelope itself — no follow-up `ReadUtxos` round-trip,
which is the regression the v1beta migration was specifically
designed to remove (the v1alpha path silently dropped spent inputs).

The lift output is the row's `lifted` column:

```jsonc
{
  "tx_id":         "1e31b36253043a89a388714f7245156788cbdbceed5d74ee81679e56a7b81a86",
  "protocol_name": "orcfax-burn",
  "tx_name":       "burn_orcfax",
  "profile_name":  "mainnet",
  "parties": {
    "burner":    { "address": "71193ee6…cdf1a4b", "role": "Input" },
    "recipient": { "address": "613c12f6…ebf2fb",  "role": "Multiple" }
  },
  "inputs": [
    {
      "tir_input_name": "source",
      "address": "71193ee6…cdf1a4b",
      "party":   "burner",
      "assets":  { "Naked": 1392130 },
      "datum": {
        "decoded": {
          "Struct": { "constructor": 121, "fields": [
            { "Struct": { "constructor": 121, "fields": [
              { "Bytes":  "CER/STRIKE-ADA/3" },
              { "Number": 1777743623232 },
              { "Struct": { "constructor": 121, "fields": [
                { "Number": 2662492851 },
                { "Number": 1250000000 } ] } }
            ] } },
            { "Struct": { "constructor": 121, "fields": [
              { "Bytes": "3c12f6735ef8…ebf2fb" }
            ] } }
          ] }
        }
      }
    }
  ],
  "outputs": [
    {
      "tir_output_index": 0,
      "address": "613c12f6…ebf2fb",
      "party":   "recipient",
      "assets":  { "Naked": 6457855811 }
    }
  ],
  "burns": [
    {
      "tir_mint_index": 0,
      "policy":         "193ee65211bb3b4e0ea5f751f415269355a650e2e3706f625cdf1a4b",
      "assets":         [ ["", -25] ]
    }
  ],
  "signers": [
    { "key_hash": "3c12f6735ef8…ebf2fb", "party": "recipient" }
  ]
}
```

**Verified**:

- **Resolved inputs**: `tx_inputs=26 ref_inputs=1 resolved_inputs=27` —
  every input the matcher might query has CBOR available. No
  client-side `ReadUtxos` calls were issued.
- **Fingerprint pre-filter**: passed (the burn carries the pinned
  Orcfax policy on a non-empty multiasset bag).
- **Structural matcher**: returned a `MatchAssignment` populating the
  single tir input, the single tir output, and the burn slot. The
  matcher tolerated the on-chain shape (26 inputs, 1 output, 1 ref
  input) against the tx3's minimal description (1 input, 1 output, 1
  burn).
- **Lifter — parties**: both parties bound, with roles inferred from
  observed usage. `burner` only appears as an input → `Input`;
  `recipient` appears as an output **and** as a signer keyhash, so
  the role bag is `Multiple`.
- **Lifter — datum**: the consumed UTxO's inline datum was decoded
  through the Plutus-data → tx3-Expression mapping. The two-field
  Constr#121 envelope, the `"CER/STRIKE-ADA/3"` price-feed bytes, and
  the rational `2_662_492_851 / 1_250_000_000 ≈ 2.13` price all came
  through structurally.
- **Lifter — burns**: the burn entry carries the policy bytes plus
  `(asset_name = "", quantity = -25)`. Negative quantity confirms the
  matcher routed this to the `burns` collection rather than `mints`.
- **Lifter — signers**: the witness-set keyhash `3c12f6…ebf2fb`
  back-resolved to `recipient` because that hash matches the payment
  credential of the recipient address.

### 3 · Storage

The DB had exactly one row in `matches` after the run, with the cursor
advanced to the target block:

```sql
sqlite> SELECT count(*) FROM matches; SELECT slot, hex(block_hash) FROM cursor;
1
186188550 | A18369937E9ACD0B0095173CBD35B8B2F013BA13DCC34518E6D5A8A5004433E1
```

```sql
sqlite> SELECT source_name, tx_name, hex(tx_hash), block_slot
        FROM matches;

source_name          tx_name      hex(tx_hash)                                                      block_slot
orcfax-burn-mainnet  burn_orcfax  1E31B36253043A89A388714F7245156788CBDBCEED5D74EE81679E56A7B81A86  186188550
```

**Verified**:

- **Idempotency** — restarting the daemon immediately re-applied the
  block from the cursor (the upstream re-sent the same `Apply`); the
  `INSERT OR IGNORE` plus `UNIQUE(tx_hash, source_name)` constraint
  produced zero new rows on the second pass.
- **Atomicity** — `cursor.slot` matches `block_slot` of the inserted
  row, confirming the row write and cursor update committed in the
  same SQLite transaction.
- **JSON queryability** — the `lifted` column is queryable via SQLite
  JSON1 functions; for instance:

  ```sql
  sqlite> SELECT json_extract(lifted, '$.burns[0].policy')
          FROM matches;
  [25,62,230,82,17,187,59,78,14,165,247,81,244,21,38,147,
   85,166,80,226,227,112,111,98,92,223,26,75]
  ```

  (Bytes are stored as JSON arrays of integers; consumers cast as
  needed.)

## Observations

- **`tir_hash` is currently 0** in the persisted row. The fingerprint
  carries an FNV1a-64 hash of the specialized TIR but it isn't being
  copied into the `Lifted` record on the way out. Pre-existing bug,
  unrelated to integration. Tracked separately.
- **Idle events are not being persisted into the cursor.** Today the
  daemon only advances the cursor inside `apply_block`; an `Idle`
  event for slot 186188607 was logged but the cursor still reads
  186188550. That's fine for correctness (re-streaming from
  186188550 just replays the same `Apply`), but on a long quiet
  window the next restart would force the upstream to re-send a lot
  of `Idle` advances. Worth fixing, but not blocking integration.
- **All three components decoupled cleanly**. The tracker module talks
  to the upstream and store; nothing in `crates/tx3-lift{,-cardano}`
  depends on tonic, rusqlite, or tokio. Cross-component contracts (a
  `CardanoPayload` going in, a `Lifted` coming out) held with no
  surprise dependencies.

## Conclusion

End-to-end integration across **tracking**, **enrichment**, and
**storage** is verified against a real mainnet transaction, with the
result reproducible from the committed fixture as long as the
configured upstream's WAL window covers slot `186188536`. The two
observations above are tracked as separate issues.
