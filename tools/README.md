> **Experimental. Testnet-10 only.**

# oneword tools

- `oneword.rs`: a Rust binary that builds every oneword spend path, signs locally and writes one JSON file per transaction (Borsh hex included). It never prints a key. It also builds the deliberately invalid variants used in the test run, signs off-chain statements (`sign`), and runs an offline `selftest`.
- `submit1.mjs`: sends one of those files to a synced testnet-10 node (`node submit1.mjs <row.json> <log.jsonl>`). It refuses a node that is not testnet-10 or not synced.
- `waitage.mjs`: waits until a transaction has left the mempool, then until the DAA score has moved on by `window` plus a margin (`node waitage.mjs <txid> <window> [margin]`).

## Build

1. Check out argent-lang/kcc20-reference at `c8a087117735a1f87c5c6d115fcddeaf2562c784` (it pins Argent `9a9f4b10`, Silverscript v1.0.0 and Rust 1.94.1).
2. Copy `../contracts/oneword.ag` to `contracts/oneword.ag` and `oneword.rs` to `src/bin/oneword/main.rs`.
3. In `Cargo.toml` add a binary and three dependencies:

```toml
[[bin]]
name = "oneword"
path = "src/bin/oneword/main.rs"

[dependencies]
kaspa-addresses = { git = "https://github.com/kaspanet/rusty-kaspa", rev = "a41a333b08848f41bf737b72592e463a6011b8ac" }
serde_json = "1"
blake2b_simd = "1.0.5"
```

4. `cargo build --bin oneword`, then `oneword selftest` (no node or key needed). It should report 22 passed and 0 failed.

`../generated/Bond.sil` and `../generated/Unbonding.sil` are the Silverscript that Argent generated from the contract, kept here for review. `oneword info` prints the artifact id; it should be `06be3b66f92da3f331d09d23e40f07b5ab7c7e4990a72a2e9f6bfe109114d9ee`.

## Environment

| Variable | Meaning |
| --- | --- |
| `OW_PAYER_KEY` | file with one 64-hex secret: pays fees and funds bonds; also the default beneficiary (keep it outside any repo) |
| `OW_PAYER_ADDRESS` | its `kaspatest:` P2PK address; the tool refuses a mismatch or a mainnet address |
| `OW_ATTESTER_KEY` | file with the attester's secret (signs statements, exit, rejoin, withdraw) |
| `OW_FORGER_KEY` | file with an unrelated key, used only by the invalid variants |
| `OW_STATE` | JSON state file: payer coin, bonds, payouts |
| `OW_OUT` | output folder; each command writes `<label>.json` with the transaction and the `next` state to use if it lands |
| `OW_BOND`, `OW_LABEL` | which bond (default `A`) and the file label (default `<bond>-<command>`) |
| `OW_RATE` | sompi per gram (default 100; TN10 nodes refused 10 per gram of compute mass) |
| `OW_BOND_VALUE`, `OW_UNBOND`, `OW_BOUNTY`, `OW_MIN_TOPUP`, `OW_DOMAIN_SEED` | genesis parameters (defaults: 1 tKAS, 300 DAA, 20 %, 0.2 tKAS, `oneword/v1/<bond>`) |
| `OW_EPOCH`, `OW_TEXT_A`, `OW_TEXT_B`, `OW_OTHER_SEED` | statements used by the invalid slash variants |
| `OW_EXPECT` | override the expected verdict of a valid build (for example `reject` for a send before the delay) |
| `KASPA_WASM`, `OW_URL` | for the `.mjs` scripts: path to the kaspa-wasm32-sdk `kaspa.js`, and the wRPC Borsh URL of your node |

## Commands

`genesis <funding-file|payer>`, `topup <sompi>`, `exit`, `rejoin`, `withdraw`, `slash <epoch> <text-a> <text-b>`, `sign <epoch> <text> <domain-hex>`, and the invalid variants `bad-topup`, `bad-exit`, `bad-shrink`, `bad-withdraw`, `bad-same`, `bad-epoch`, `bad-forger`, `bad-domain`, `bad-greedy`, `bad-dest`, `bad-twoin`.

A statement body is `blake2b(text)`. The tool's `slash` signs both texts with the attester key, which is what a drill needs. A real slasher does not have that key: it collects two published signatures and passes them in.

Use the `next` state only after the node has accepted the transaction and it has really landed. Never adopt the `next` state of a file you built earlier and are only sending now: it is a snapshot from build time (the test run did this once, see the main README).
