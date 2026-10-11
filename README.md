> **Experimental. Not advice. Testnet-10 only.** tKAS has no value. [DISCLAIMER.md](DISCLAIMER.md)

# oneword

A bond on Kaspa Testnet-10 that anyone can slash if its owner says two different things about the same moment.

An **attester** (an indexer, a bot, a shop, an oracle, a sequencer) locks tKAS in one covenant output and signs short statements off chain:

```
statement = blake2b(domain || epoch (8 bytes, little endian) || body (32 bytes))
```

The promise is **one body per epoch**. If two valid signatures ever exist for the same `domain` and `epoch` with different bodies, that pair is the whole proof. Anyone can hand it to the covenant. No referee, no vote, no server. The covenant then pays at least 80 % of the bond to a fixed **beneficiary** and lets the slasher keep up to 20 % as a bounty.

Everything above is enforced by the covenant on L1.

## Why

Most "signed" numbers in this ecosystem cost nothing to contradict later. Our own storm write-ups are an example: a bot posts a headline, and nothing stops it, or whoever holds its key, from posting a different one for the same storm somewhere else. A signature proves who said something, not that they said only one thing.

oneword attaches a price to saying two things. That is useful wherever a reader acts on "the" value for a slot:

- **storm headlines and indexer counts**: one number per storm or per checkpoint;
- **price lists for the 1984 square's shops**: one list per day, so a shop cannot quote different prices to different buyers for the same day;
- **oracles and sequencers for future dapps**: one answer per round, one batch per height.

It is a different branch from [hotcell](https://github.com/STP-KAS/hotcell) and [feetank](https://github.com/STP-KAS/feetank): no fees, no congestion, no minting, no rate limits. It is about credible statements.

## Spend paths

Two actors share one state: `Bond` (live) and `Unbonding` (the attester has announced an exit).

| Entry | Actor | Who | Rule enforced by the covenant |
| --- | --- | --- | --- |
| `topup()` | Bond | anyone | successor is the same Bond with at least `value + min_topup` |
| `exit(sig)` | Bond | attester | successor is an `Unbonding` with the same state and exactly the same value |
| `rejoin(sig)` | Unbonding | attester | successor is a `Bond` with the same state and exactly the same value |
| `withdraw(sig)` | Unbonding | attester | the `Unbonding` output is at least `unbond_delay` DAA old; no successor, the attester picks the outputs |
| `slash(epoch, body_a, sig_a, body_b, sig_b)` | Bond and Unbonding | anyone | `body_a != body_b`; both are valid BIP-340 signatures by `attester` over the statement digest with this bond's `domain` and the same `epoch`; the transaction has exactly one input; output 0 pays `beneficiary` (P2PK) at least `value - value * bounty_pct / 100`; no successor |

State: `attester`, `domain` (32 bytes chosen at genesis), `beneficiary`, `bounty_pct`, `unbond_delay`, `min_topup`. None of it can change after genesis. `unbond_delay` is a relative DAA lock (`this.ageDaa`). At 10 blocks per second, 300 DAA is about 30 seconds; the test run used that so the drill fits in two minutes. A real bond needs days.

## What is enforced and what is not

Enforced on L1:

- a pair of conflicting statements for one epoch, signed by the attester under this bond's domain, releases the bond, from either actor;
- at least `(100 - bounty_pct)` % goes to the beneficiary; the slasher cannot take more;
- one bond per slash transaction, so one beneficiary output cannot be counted for two bonds;
- only the attester can exit, rejoin or withdraw, and only after the delay;
- exit and rejoin cannot skim the bond; top-ups have a floor.

Not enforced, or only convention:

- **Truth.** A single false statement is never punished. oneword makes a lie *consistent*, not true. The bond only buys "this attester said exactly one thing for epoch E".
- **Publication.** Equivocation is only punished if both statements reach someone who slashes. An attester who shows a second statement privately to one victim is caught only if that victim slashes.
- **What an epoch means.** The covenant sees 8 bytes. Whether that is a storm date, a DAA checkpoint or a round number is a convention between the attester and its readers. So is the text behind a body hash.
- **Corrections.** A legitimate correction for the same epoch is, to the covenant, equivocation. Publish corrections under a new epoch that refers to the old one.
- **Who the attester is.** The bond binds a public key, not a person or a service.
- **That a bond still backs a statement.** A reader must check that the bond output is live, that it is a `Bond` and not an `Unbonding`, and how big it is. The covenant cannot stop anyone from relying on a dead bond.

## Attack and grief vectors

- **Lie, then leave.** The attester signs two statements, then calls `exit` and hopes nobody notices before `unbond_delay` runs out. `slash` works from `Unbonding` for exactly this reason (tested on chain: bond C). `unbond_delay` must be longer than the time readers need to notice and slash, plus a censorship margin.
- **Self-slash.** An attester who has been caught can slash itself first and keep the bounty. The beneficiary still gets at least `100 - bounty_pct` %. Keep `bounty_pct` small. It only has to cover the slasher's fee and effort.
- **Bounty front-running.** `slash` needs no signature, so whoever sees the transaction first (another node, a miner) can copy the proof and redirect output 1 to itself. The beneficiary's share is safe; the bounty is not. A commit-reveal slash would fix this at the cost of a second transaction. Not built.
- **Censorship.** Miners who do not include slashes for longer than `unbond_delay` let the attester walk away. On mainnet the delay should be long.
- **Domain reuse.** Two bonds of the same attester with the same `domain` can both be slashed with one proof (one transaction each). Use a fresh domain per bond. Statements made under another bond's domain do not count (tested: `bad-domain`).
- **Bond too small.** If one inconsistent statement earns more than the bond, oneword does not deter it. Size the bond to the value of what readers do with the statement.
- **Grief by top-up.** Anyone can add to a live `Bond`. That only makes the stake bigger, and it does not touch the unbonding clock: there is no top-up on `Unbonding`, so nobody can reset an exit that is under way.
- **Stolen attester key.** The thief can equivocate on purpose and slash the bond, getting the bounty. The beneficiary still gets its share. The thief can also start an exit; the real attester can `rejoin`, and the thief then has to wait out the delay again.

## Is a covenant the right tool?

Yes for the core claim. Without introspection there are two older ways to punish equivocation, and both are weaker:

- **Key-leaking signatures.** "Liar, Liar, Coins on Fire!" (Ruffing, Kate, Schröder, ACM CCS 2015) uses double-authentication-preventing signatures, so two conflicting statements leak the signing key and anyone can take the deposit. Works without covenants, but the whole deposit goes to whoever is fastest, usually a miner or the attester itself. oneword routes the main share to a fixed beneficiary.
- **A referee.** A slashable bond that pays on a referee's signed verdict, as in [Kaspa-World-Eater/quorum](https://github.com/Kaspa-World-Eater/quorum) (proposed as Draft KCC-4 "Slashable Bond" in [kaspanet/kccs#29](https://github.com/kaspanet/kccs/pull/29), not adopted), can punish any lie the referee recognises, not only equivocation. It needs a trusted referee. oneword needs none, and in exchange it can only punish self-contradiction. The two compose: a referee bond for truth, a oneword bond for consistency.

Proof-of-stake chains slash validators for double signing in the same spirit. On Kaspa, [biryukovmaxim/kaspa-xmss](https://github.com/biryukovmaxim/kaspa-xmss) uses a covenant to stop its own wallet from signing one index twice; that prevents reuse inside one lineage rather than punishing it. As far as we found (web search, 11 Oct 2026), there is no published equivocation bond on Kaspa covenants yet.

No for some uses. If readers can simply wait for a statement to be anchored on chain (a payload in a Kaspa transaction), the chain already orders it and "the first anchored statement wins" needs no bond. oneword helps when statements travel off chain and readers act on them before, or instead of, any anchor.

## Design choices and rejected alternatives

Before building, six ideas in different domains were scored from 1 (bad) to 5 (good). For "enforceable" a 5 means the covenant can enforce what the idea needs; for "build risk" a 5 means low risk. Fees, congestion, rate limits and minting were out of scope on purpose.

| Idea | Domain | New | Useful | Enforceable on TN10 | Build risk | Verdict |
| --- | --- | --- | --- | --- | --- | --- |
| **oneword** (equivocation-slashable attester bond, no referee) | attestation, data | 4 | 3 | 4 | 4 | **built** |
| Guardian recovery vault (2-of-3 guardians rotate the owner key after a delay the owner can veto, heir claims after inactivity) | recovery, inheritance | 2 | 4 | 5 | 4 | Lost on novelty. Kaspa Safe (kaspaforge.org; its blog says it runs on mainnet covenants) already has a delayed vault with an alarm-key cancel, check-ins and heir paths, and DeadManSwitch covenants are live on testnet. Lesson kept: a permissionless top-up would let anyone reset a dead-man clock. |
| Escrow for the 1984 square's shops (buyer, seller, arbiter, deadlines) | commerce | 1 | 4 | 2 | 4 | Kaspa Escrow, the kascov lab escrow on TN10 and others exist. A covenant cannot see a delivery, so the arbiter carries the trust anyway. |
| Commit-reveal coin flip with forfeit on timeout | games, randomness | 2 | 2 | 4 | 3 | Known since early Bitcoin research (Andrychowicz et al. 2014). Little use beyond gambling. |
| Token-weighted vote with hotcell (KCC-20 composition) | governance | 3 | 3 | 1 | 1 | Tokens move between UTXOs, so double voting needs tokens locked into vote outputs, and counting many outputs hits the input and mass limits. Too big. |
| Name registry | naming | 1 | 3 | 3 | 2 | DOTK `.k` names and KNS already exist. |

Inside oneword:

- **Fixed-width digest** (`domain` 32 bytes, `epoch` 8 bytes, `body` 32 bytes) so two different statements can never encode to the same bytes.
- **Body is a hash.** The covenant never sees the text, which keeps the script small and the statement any length.
- **Two-step exit** (`exit`, then `withdraw` after the delay) instead of a direct withdraw, so there is always a window to slash. `rejoin` lets an attester undo an exit, or undo one a thief started.
- **Fixed beneficiary, small bounty** instead of "slasher takes all", against self-slashing and front-running.
- **One input per slash** instead of an output index per input. Simpler, and it rules out counting output 0 twice.
- **No parameter changes.** A bond whose domain or beneficiary can change cannot be relied on. Make a new bond instead.

## Throughput and mass

- A bond is one output. Statements cost nothing on chain; only the life cycle and the slash are transactions.
- Scripts: `Bond` 662 bytes, `Unbonding` 669 bytes. Measured on TN10 (genesis at 10 sompi/gram, the rest at 100 sompi/gram, which TN10 nodes required for compute mass):

| Path | Compute mass | Storage mass | Fee |
| --- | --- | --- | --- |
| genesis (1 tKAS bond) | 2,083 | 42,901 | 0.0043 tKAS |
| topup (0.2 tKAS) | 2,807 | 1,847 | 0.0028 tKAS |
| exit | 4,354 | 2,009 | 0.0044 tKAS |
| rejoin | 4,354 | 2,120 | 0.0044 tKAS |
| withdraw | 2,303 | 0 | 0.0023 tKAS |
| slash (from Bond or Unbonding) | 3,847 to 3,854 | 32,039 | 0.0320 tKAS, paid from the bounty |

The slash is the expensive path because the bounty output is small next to the bond (KIP-9 storage mass). A bigger bond or a bigger bounty share makes it cheaper per tKAS.

## How a developer uses it

1. **Attester:** pick a fresh `domain`, a `beneficiary` (the readers you want to compensate, or a burn script if you prefer none), `bounty_pct` (small, for example 5 to 20), and an `unbond_delay` far longer than your readers' reaction time. Create the bond with the `genesis` command of [tools/oneword.rs](tools/oneword.rs).
2. Publish the bond's covenant id, `domain` and what an epoch means for you.
3. For each epoch, sign exactly one body: `oneword sign <epoch> <text> <domain-hex>` prints the statement digest and the signature. Publish text, epoch and signature together.
4. **Reader:** recompute `body = blake2b(text)` and the digest, check the signature against the bond's `attester`, then check that the bond output is live, is a `Bond` (not `Unbonding`), and is big enough for what you are about to do. Keep every statement you see.
5. **Anyone who sees two bodies for one epoch:** build a `slash` with both signatures, the bond as the only input, output 0 to the beneficiary with at least its share, output 1 to yourself. Send it quickly; others can copy it.
6. **To leave:** `exit`, wait `unbond_delay`, `withdraw`. To stay after all, `rejoin`.

## Test run on TN10 (11 Oct 2026, 19:16 to 19:18 CEST)

Deployed by Grok Bot from its own TN10 wallet through a synced local TN10 node. Parameters for all three bonds: value 1 tKAS, `bounty_pct` 20, `unbond_delay` 300 DAA, `min_topup` 0.2 tKAS. The attester and the unrelated "forger" key were throwaway keys made for this run. The Grok Bot TN10 key paid the fees and was both beneficiary and slasher, so every tKAS came back except fees. All keys were destroyed after the run. The equivocating statements were made on purpose for the drill; they are not real claims.

| Field | Value |
| --- | --- |
| App | `OneWord`, artifact `06be3b66f92da3f331d09d23e40f07b5ab7c7e4990a72a2e9f6bfe109114d9ee` |
| Launch path | `launch::oneword-v1` |
| Bond A (life cycle) | covenant `1e92277e4df3eaef4cb3fd8fe0a7d28dfd076a06d41a4fa03ad3795b364e7ea9`, withdrawn |
| Bond B (slashed while live) | covenant `f7c4c5e758a5bf3d2f9edc3ea689eb79913d2f899a9af6ef6cf8df5f34bc6af0`, slashed in `87d728028ea3779c61553242653b4c18a7036f600adce02d5f3d0d23229fe0e6` |
| Bond C (slashed while unbonding) | covenant `8a07e77457a6b4e151bc612faa1e3f6ab737983d46c7aba78697aaec2b79dc45`, slashed in `93699eb9a5ae36f9c1b2243a9e0eee30187673e966cf5b352bbb8c25b10efe20` |
| Status | all three closed. Deploy your own. |

Every path ran on chain. Each reject case is a valid transaction with exactly one thing changed, and the unchanged version passes. The full log is [results/landed.jsonl](results/landed.jsonl).

**Landed (11):** three geneses; bond A: top-up 0.2 tKAS, exit, rejoin, exit, withdraw; bond B: slash with two statements for epoch 20261011 ("tn10-storm-2026-10-11 peak complete minute 3583.1 tx/s" against "… 3999.9 tx/s"); bond C: exit, then slash with two statements for epoch 2. Each slash paid 0.8 tKAS to the beneficiary and 0.168 tKAS to the slasher after the 0.032 tKAS fee.

**Rejected by the covenant script on the node, as designed (12):**

- top-up 1 sompi below `min_topup`;
- exit signed by the forger key;
- exit signed by the attester but skimming 0.01 tKAS off the bond;
- withdraw after the delay signed by the forger key;
- slash with the same statement twice;
- slash with two statements from different epochs;
- slash whose second statement was signed by the forger key;
- slash with a real equivocation made under bond A's domain, presented to bond B;
- slash paying the beneficiary 1 sompi less than its share;
- slash paying the beneficiary share to another key, once from `Bond` and once from `Unbonding`;
- slash with a second input added.

**Rejected by the relative lock, as designed (1):** bond A's withdraw sent right after the exit landed ("one of the transaction sequence locks conditions was not met"). The same bytes were sent again after 300 DAA and landed.

**Mistakes in the run, owned:**

- The first try of the top-up and two exits went out at 10 sompi/gram and TN10 refused them as below the minimum fee for their compute mass (100 sompi/gram). They were rebuilt at 100 and landed. The three invalid variants sent in that first try were refused by the script anyway, and were sent again at the right fee.
- The tool's state file was overwritten once with a stale snapshot (the early withdraw was built before bonds B and C moved). Nothing on chain was affected; the state was rebuilt from the later transactions and every resulting coin was checked on the node.

**Offline selftest:** `oneword selftest` runs 22 cases (11 valid, 11 invalid) with public demo keys and made-up coins: 22 passed, 0 failed.

**Cost:** 0.1016 tKAS in fees for the 11 landed transactions. Nothing stays locked. The rejected transactions cost nothing.

Intentions are good; thought process is questionable. STP remains delusional. Si vis pacem, para bellum.
