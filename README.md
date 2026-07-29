# tx-processor

Toy payments engine: reads a CSV of deposits/withdrawals/disputes/resolves/chargebacks, applies them per client, writes resulting balances as CSV.

## Usage

```
cargo run -- transactions.csv > accounts.csv
```

Rejected/malformed rows are logged to stderr and skipped, not fatal. Only a bad CLI arg, an unopenable file, or a write failure exits non-zero.

## Design

- **`Ledger::apply(&mut self, tx) -> Result<(), LedgerError>`** is the single mutation point: deterministic, no I/O, no hidden state. Not immutable-functional-pure (it mutates in place rather than returning a new `Ledger`) — chosen over a clone-and-return approach because the assignment calls out large datasets, and cloning the whole ledger per transaction doesn't scale. It *is* atomic: every arm validates and computes fully before writing anything back, so a rejected transaction leaves the ledger unchanged rather than half-applied.
- **Streaming I/O.** CSV reading/writing goes through the `csv` crate's incremental reader/writer via iterators, no intermediate `Vec` — memory use doesn't scale with file size.
- **Types carry invariants.** `ClientId`/`TxId`/`Amount`/`Balance` are `nutype` newtypes. `Amount` (a transaction's face value) is non-negative, ≤4 decimals. `Balance` (available/held/total) allows the same precision but *not* non-negativity — `available` can go negative when a deposit is disputed after its funds were already withdrawn.

## Assumptions

- Only deposits can be disputed. A dispute holds funds in `held`; a withdrawal's funds already left and aren't held anywhere, so disputing one would fabricate money. Rejected with `WithdrawalNotDisputable`.
- A locked account rejects everything afterward, not just further disputes.
- Unknown/mismatched dispute references are ignored (logged, not fatal), per the assignment's "assume partner error." This includes re-disputing a transaction that's already under dispute (`AlreadyDisputed`) — the spec doesn't address double-dispute directly, but it falls under the same "malformed reference from the partner" umbrella.
- Duplicate tx IDs are rejected rather than silently overwriting history.

## Testing

- **Types** — `nutype` validation rejects malformed amounts at construction; nothing downstream re-checks.
- **Property-based (`proptest`, `src/ledger.rs`)** — random transaction sequences check: a rejected `apply` never mutates the ledger and a locked account rejects everything; money is conserved (`Σ total == Σ deposits − Σ withdrawals − Σ chargebacks`); `held` never goes negative; replaying the same sequence on two fresh ledgers is deterministic; dispute/resolve preserve `total`, chargeback reduces it by exactly the disputed amount and locks the account.
- **Golden / end-to-end (`tests/golden.rs`)** — spawns the actual compiled binary against fixtures in `tests/fixtures/`, diffs stdout byte-for-byte against a blessed expected file. Exact-text comparison works because `Ledger` stores accounts in a `BTreeMap`, not a `HashMap`, so output order is deterministic. Also checks missing-arg/nonexistent-file failure paths.

Run with `cargo test`.

## With more time

- **Shard by client ID.** `apply` only ever touches its transaction's own client, so partitioning clients across N independent `Ledger`s and fanning transactions out by `client_id % N` would parallelize processing, as long as each shard preserves per-client order. Not implemented.
- **Bound `history`.** It currently retains every deposit/withdrawal for the process lifetime, since a dispute could reference something arbitrarily old. A long-running server would want to evict entries past a real chargeback window (e.g. 120 days) to bound memory. Not implemented.

## Caveats

- "Streaming keeps memory flat" is architectural (no unbounded buffering in the I/O path, verified by a test that checks only a prefix of a 100k-row input is read before the first record comes out) — not measured against an actual large-memory run.
- `history` is unbounded today, as noted above.
