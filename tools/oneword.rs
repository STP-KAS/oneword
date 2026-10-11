//! oneword tool: builds every oneword spend path for testnet-10 drills.
//!
//! oneword is a bond that anyone can slash once its attester signs two
//! different statements for the same epoch. Each command builds ONE
//! transaction (or one deliberately invalid transaction for the reject cases)
//! and writes it to `$OW_OUT/<label>.json` together with the state to adopt if
//! the node accepts it. Keys are read from mode-600 files named in env vars and
//! are never printed.
//!
//! Commands (the bond is chosen with OW_BOND, default "A"):
//!   genesis <funding-file|payer>  create a bond
//!   topup <sompi>                 anyone adds to a live bond
//!   exit | rejoin                 attester starts / stops unbonding
//!   withdraw                      attester takes an unbonding bond after the delay
//!   sign <epoch> <text>           off-chain statement (prints digest and signature)
//!   slash <epoch> <text-a> <text-b>  anyone, with two conflicting statements
//!   bad-topup | bad-exit | bad-shrink | bad-withdraw | bad-same | bad-epoch |
//!   bad-forger | bad-domain | bad-greedy | bad-dest | bad-twoin
//!                                 invalid variants the covenant must reject
//!   selftest                      offline run of all paths with demo keys

use std::fs;
use std::str::FromStr;

use argent::artifact::Artifact;
use argent::build_file;
use argent_runtime::{
    CovenantOutput, EntryCall, InputSigScript, TxBuilder, TxContext, args, execute_input_with_covenants, state,
};
use kaspa_addresses::{Address, Prefix, Version};
use kaspa_consensus_core::Hash;
use kaspa_consensus_core::hashing::sighash::{SigHashReusedValuesUnsync, calc_schnorr_signature_hash};
use kaspa_consensus_core::hashing::sighash_type::SIG_HASH_ALL;
use kaspa_consensus_core::mass::MassCalculator;
use kaspa_consensus_core::tx::{
    CovenantBinding, MutableTransaction, ScriptPublicKey, Transaction, TransactionId, TransactionOutpoint, UtxoEntry,
};
use secp256k1::{Keypair, Message, Secp256k1, SecretKey};
use serde_json::{Value, json};

type R<T> = Result<T, Box<dyn std::error::Error>>;
type St = std::collections::BTreeMap<String, ArtifactValueAlias>;
type ArtifactValueAlias = argent_runtime::ArtifactValue;

const KAS: u64 = 100_000_000;
const LAUNCH_PATH: &str = "launch::oneword-v1";
const STORAGE_LIMIT: u64 = 100_000;

struct Coin {
    outpoint: TransactionOutpoint,
    entry: UtxoEntry,
}

struct Ready {
    tx: Transaction,
    entries: Vec<UtxoEntry>,
    storage: u64,
    compute: u64,
    transient: u64,
    fee: u64,
}

#[derive(Clone)]
struct Params {
    attester: [u8; 32],
    domain: [u8; 32],
    beneficiary: [u8; 32],
    bounty_pct: i64,
    unbond_delay: i64,
    min_topup: i64,
}

impl Params {
    fn state(&self) -> St {
        state! {
            attester: self.attester.to_vec(),
            domain: self.domain.to_vec(),
            beneficiary: self.beneficiary.to_vec(),
            bounty_pct: self.bounty_pct,
            unbond_delay: self.unbond_delay,
            min_topup: self.min_topup
        }
    }
    fn json(&self) -> Value {
        json!({"attester": hex(&self.attester), "domain": hex(&self.domain), "beneficiary": hex(&self.beneficiary),
               "bountyPct": self.bounty_pct, "unbondDelay": self.unbond_delay, "minTopup": self.min_topup})
    }
    fn from_json(v: &Value) -> R<Self> {
        Ok(Self {
            attester: arr32(s(v, "attester")?)?,
            domain: arr32(s(v, "domain")?)?,
            beneficiary: arr32(s(v, "beneficiary")?)?,
            bounty_pct: i(v, "bountyPct")?,
            unbond_delay: i(v, "unbondDelay")?,
            min_topup: i(v, "minTopup")?,
        })
    }
}

struct Ctx {
    payer: Keypair,
    payer_pub: [u8; 32],
    out: String,
    rate: u64,
    state: Value,
    bond: String,
}

fn main() -> R<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cmd = argv.first().cloned().ok_or("usage: oneword <cmd> [args]")?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let art = build_file(root.join("contracts/oneword.ag"), root.join("build/oneword"))?;
    if cmd == "info" {
        println!("app {} artifact {}", art.app, art.id);
        for (name, contract) in &art.sil_abi.contracts {
            println!("  {name}: {} bytes", contract.compiled.bytecode.len());
        }
        return Ok(());
    }
    if cmd == "selftest" {
        return selftest(&art);
    }
    if cmd == "sign" {
        let attester = load_key("OW_ATTESTER_KEY", None)?.0;
        let domain = arr32(argv.get(3).ok_or("usage: sign <epoch> <text> <domain-hex>")?)?;
        let epoch: u64 = argv.get(1).ok_or("epoch")?.parse()?;
        let text = argv.get(2).ok_or("text")?;
        let (digest, sig) = sign_statement(&attester, &domain, epoch, &body_of(text));
        println!("{}", json!({"epoch": epoch, "text": text, "body": hex(&body_of(text)), "statement": hex(&digest), "sig": hex(&sig),
            "attester": hex(&attester.x_only_public_key().0.serialize())}));
        return Ok(());
    }
    let (payer, payer_pub) = load_key("OW_PAYER_KEY", Some("OW_PAYER_ADDRESS"))?;
    let out = std::env::var("OW_OUT").map_err(|_| "OW_OUT")?;
    fs::create_dir_all(&out)?;
    let rate = std::env::var("OW_RATE").ok().map(|v| v.parse()).transpose()?.unwrap_or(100u64);
    let state_path = std::env::var("OW_STATE").map_err(|_| "OW_STATE")?;
    let state: Value = serde_json::from_str(&fs::read_to_string(&state_path)?)?;
    let bond = std::env::var("OW_BOND").unwrap_or_else(|_| "A".into());
    let ctx = Ctx { payer, payer_pub, out, rate, state, bond };
    let builder = TxBuilder::new(&art)?;
    let label = std::env::var("OW_LABEL").unwrap_or_else(|_| format!("{}-{cmd}", ctx.bond));
    run_cmd(&ctx, &builder, &label, &cmd, &argv)
}

fn run_cmd(ctx: &Ctx, b: &TxBuilder<'_>, label: &str, cmd: &str, argv: &[String]) -> R<()> {
    let arg = |k: usize| argv.get(k).cloned().ok_or_else(|| format!("{cmd}: missing argument {k}"));
    match cmd {
        "genesis" => genesis(ctx, b, label, &arg(1)?),
        "topup" => {
            let ready = topup(ctx, b, arg(1)?.parse()?)?;
            let mut next = with_bond(ctx, &ready.tx, 0, "Bond")?;
            next["payer"] = coin_json(&ready.tx, 1);
            emit(ctx, label, "topup", &ready, next)
        }
        "exit" => {
            let ready = exit_or_rejoin(ctx, b, true, None)?;
            let mut next = with_bond(ctx, &ready.tx, 0, "Unbonding")?;
            next["payer"] = coin_json(&ready.tx, 1);
            emit(ctx, label, "exit", &ready, next)
        }
        "rejoin" => {
            let ready = exit_or_rejoin(ctx, b, false, None)?;
            let mut next = with_bond(ctx, &ready.tx, 0, "Bond")?;
            next["payer"] = coin_json(&ready.tx, 1);
            emit(ctx, label, "rejoin", &ready, next)
        }
        "withdraw" => {
            let ready = withdraw(ctx, b, None)?;
            let mut next = closed(ctx, "withdrawn")?;
            push_payout(&mut next, &ready.tx, 0)?;
            emit(ctx, label, "withdraw", &ready, next)
        }
        "slash" => {
            let (epoch, a, bb) = (arg(1)?.parse()?, arg(2)?, arg(3)?);
            let att = load_key("OW_ATTESTER_KEY", None)?.0;
            let p = bond_params(ctx)?;
            let (_, sa) = sign_statement(&att, &p.domain, epoch, &body_of(&a));
            let (_, sb) = sign_statement(&att, &p.domain, epoch, &body_of(&bb));
            let ready = slash(ctx, b, epoch, &body_of(&a), &sa, &body_of(&bb), &sb)?;
            let mut next = closed(ctx, "slashed")?;
            push_payout(&mut next, &ready.tx, 0)?;
            push_payout(&mut next, &ready.tx, 1)?;
            next["bonds"][&ctx.bond]["slash"] = json!({"epoch": epoch, "textA": a, "textB": bb});
            emit(ctx, label, "slash", &ready, next)
        }
        c if c.starts_with("bad-") => bad(ctx, b, label, c),
        _ => Err(format!("unknown command {cmd}").into()),
    }
}

// ---------- statements ----------

fn body_of(text: &str) -> [u8; 32] {
    blake2b32(text.as_bytes())
}

fn blake2b32(data: &[u8]) -> [u8; 32] {
    let h = blake2b_simd::Params::new().hash_length(32).to_state().update(data).finalize();
    h.as_bytes().try_into().expect("32")
}

/// statement = blake2b(domain || epoch (8 bytes, little endian) || body); the attester signs it with BIP-340.
fn sign_statement(key: &Keypair, domain: &[u8; 32], epoch: u64, body: &[u8; 32]) -> ([u8; 32], Vec<u8>) {
    let mut m = Vec::with_capacity(72);
    m.extend_from_slice(domain);
    m.extend_from_slice(&epoch.to_le_bytes());
    m.extend_from_slice(body);
    let digest = blake2b32(&m);
    let sig = key.sign_schnorr(Message::from_digest(digest)).as_ref().to_vec();
    (digest, sig)
}

// ---------- valid paths ----------

fn genesis(ctx: &Ctx, b: &TxBuilder<'_>, label: &str, funding: &str) -> R<()> {
    let from_file = funding != "payer";
    let coin = if from_file { load_funding(funding, &ctx.payer_pub)? } else { payer_coin(ctx)? };
    let attester = load_key("OW_ATTESTER_KEY", None)?.1;
    let seed = std::env::var("OW_DOMAIN_SEED").unwrap_or_else(|_| format!("oneword/v1/{}", ctx.bond));
    let p = Params {
        attester,
        domain: blake2b32(seed.as_bytes()),
        beneficiary: ctx.payer_pub,
        bounty_pct: env_i("OW_BOUNTY", 20)?,
        unbond_delay: env_i("OW_UNBOND", 300)?,
        min_topup: env_i("OW_MIN_TOPUP", 20_000_000)?,
    };
    let value = env_i("OW_BOND_VALUE", KAS as i64)? as u64;
    let payer = ctx.payer;
    let ready = fit_fee(ctx.rate, |fee| {
        let change = coin.entry.amount.checked_sub(value).and_then(|v| v.checked_sub(fee)).ok_or("change")?;
        let tx = b.build(
            &TxContext::new()
                .input(coin.outpoint, coin.entry.clone(),
                    InputSigScript::with_transaction(move |tx, index| p2pk_signature(&sign(tx, index, &payer))), 0)
                .actor_genesis_output(0, LAUNCH_PATH, "Bond", p.state(), value)
                .output(p2pk(&ctx.payer_pub), None, change),
        )?;
        commit(tx, vec![coin.entry.clone()])
    })?;
    let cov = CovenantOutput::from_tx(&ready.tx, 0)?;
    let mut next = ctx.state.clone();
    if from_file {
        if let Some(old) = ctx.state.get("payer").cloned() {
            let mut spare = next.get("spare").cloned().unwrap_or_else(|| json!([]));
            spare.as_array_mut().ok_or("spare")?.push(old);
            next["spare"] = spare;
        }
    }
    next["payer"] = coin_json(&ready.tx, 1);
    if next.get("bonds").is_none() {
        next["bonds"] = json!({});
    }
    next["bonds"][&ctx.bond] = json!({"cov": cov.covenant_id.to_string(), "actor": "Bond", "params": p.json(),
        "utxo": utxo_json(&ready.tx, 0), "genesis": ready.tx.id().to_string(), "seed": seed});
    emit(ctx, label, "genesis", &ready, next)
}

fn bond_entry(ctx: &Ctx) -> R<&Value> {
    ctx.state.get("bonds").and_then(|v| v.get(&ctx.bond)).ok_or_else(|| format!("no bond {}", ctx.bond).into())
}

fn bond_params(ctx: &Ctx) -> R<Params> {
    Params::from_json(&bond_entry(ctx)?["params"])
}

fn bond_coin(ctx: &Ctx) -> R<(CovenantOutput, Params, String)> {
    let e = bond_entry(ctx)?;
    let t = &e["utxo"];
    let cov = Hash::from_str(s(e, "cov")?)?;
    let spk = ScriptPublicKey::from_vec(0, hex_decode(s(t, "script")?)?);
    let value = u(t, "value")?;
    Ok((
        CovenantOutput {
            index: u(t, "index")? as u32,
            covenant_id: cov,
            outpoint: TransactionOutpoint::new(TransactionId::from_str(s(t, "txid")?)?, u(t, "index")? as u32),
            utxo: UtxoEntry::new(value, spk, 0, false, Some(cov)),
        },
        Params::from_json(&e["params"])?,
        s(e, "actor")?.to_string(),
    ))
}

fn payer_coin(ctx: &Ctx) -> R<Coin> {
    let p = &ctx.state["payer"];
    Ok(Coin {
        outpoint: TransactionOutpoint::new(TransactionId::from_str(s(p, "txid")?)?, u(p, "index")? as u32),
        entry: UtxoEntry::new(u(p, "value")?, p2pk(&ctx.payer_pub), 0, false, None),
    })
}

fn topup(ctx: &Ctx, b: &TxBuilder<'_>, amount: u64) -> R<Ready> {
    let (bond, p, actor) = bond_coin(ctx)?;
    if actor != "Bond" {
        return Err("topup needs a live Bond".into());
    }
    let coin = payer_coin(ctx)?;
    let payer = ctx.payer;
    fit_fee(ctx.rate, |fee| {
        let change = coin.entry.amount.checked_sub(amount).and_then(|v| v.checked_sub(fee)).ok_or("change")?;
        let tx = b.build(
            &TxContext::new()
                .actor_input("Bond", p.state(), EntryCall::new("topup"), bond.outpoint, bond.utxo.clone(), 0)
                .input(coin.outpoint, coin.entry.clone(),
                    InputSigScript::with_transaction(move |tx, index| p2pk_signature(&sign(tx, index, &payer))), 0)
                .actor_output("Bond", p.state(), CovenantBinding::new(0, bond.covenant_id), bond.utxo.amount + amount)
                .output(p2pk(&ctx.payer_pub), None, change),
        )?;
        commit(tx, vec![bond.utxo.clone(), coin.entry.clone()])
    })
}

/// exit (Bond -> Unbonding) or rejoin (Unbonding -> Bond); the attester signs, the payer coin pays the fee.
fn exit_or_rejoin(ctx: &Ctx, b: &TxBuilder<'_>, exit: bool, signer: Option<Keypair>) -> R<Ready> {
    let (bond, p, actor) = bond_coin(ctx)?;
    let (from, to, entry) = if exit { ("Bond", "Unbonding", "exit") } else { ("Unbonding", "Bond", "rejoin") };
    if actor != from {
        return Err(format!("{entry} needs a {from}, bond is {actor}").into());
    }
    let key = match signer { Some(k) => k, None => load_key("OW_ATTESTER_KEY", None)?.0 };
    let coin = payer_coin(ctx)?;
    let payer = ctx.payer;
    fit_fee(ctx.rate, |fee| {
        let change = coin.entry.amount.checked_sub(fee).ok_or("change")?;
        let call = EntryCall::new(entry).args_with(move |tx, index| args!(sign(tx, index, &key)));
        let tx = b.build(
            &TxContext::new()
                .actor_input(from, p.state(), call, bond.outpoint, bond.utxo.clone(), 0)
                .input(coin.outpoint, coin.entry.clone(),
                    InputSigScript::with_transaction(move |tx, index| p2pk_signature(&sign(tx, index, &payer))), 0)
                .actor_output(to, p.state(), CovenantBinding::new(0, bond.covenant_id), bond.utxo.amount)
                .output(p2pk(&ctx.payer_pub), None, change),
        )?;
        commit(tx, vec![bond.utxo.clone(), coin.entry.clone()])
    })
}

fn withdraw(ctx: &Ctx, b: &TxBuilder<'_>, signer: Option<Keypair>) -> R<Ready> {
    let (bond, p, actor) = bond_coin(ctx)?;
    if actor != "Unbonding" {
        return Err("withdraw needs an Unbonding bond".into());
    }
    let key = match signer { Some(k) => k, None => load_key("OW_ATTESTER_KEY", None)?.0 };
    fit_fee(ctx.rate, |fee| {
        let call = EntryCall::new("withdraw").args_with(move |tx, index| args!(sign(tx, index, &key)));
        let tx = b.build(
            &TxContext::new()
                .actor_input("Unbonding", p.state(), call, bond.outpoint, bond.utxo.clone(), p.unbond_delay as u64)
                .output(p2pk(&ctx.payer_pub), None, bond.utxo.amount.checked_sub(fee).ok_or("fee")?),
        )?;
        commit(tx, vec![bond.utxo.clone()])
    })
}

/// Keyless slash: output 0 pays the beneficiary exactly bond - bounty, output 1 pays the slasher the rest minus fee.
fn slash(ctx: &Ctx, b: &TxBuilder<'_>, epoch: u64, body_a: &[u8; 32], sig_a: &[u8], body_b: &[u8; 32], sig_b: &[u8]) -> R<Ready> {
    let (bond, p, actor) = bond_coin(ctx)?;
    let bounty = bond.utxo.amount * p.bounty_pct as u64 / 100;
    let (ba, sa, bb, sb) = (body_a.to_vec(), sig_a.to_vec(), body_b.to_vec(), sig_b.to_vec());
    fit_fee(ctx.rate, |fee| {
        let call = EntryCall::new("slash").args(args!(epoch.to_le_bytes().to_vec(), ba.clone(), sa.clone(), bb.clone(), sb.clone()));
        let tx = b.build(
            &TxContext::new()
                .actor_input(actor.as_str(), p.state(), call, bond.outpoint, bond.utxo.clone(), 0)
                .output(p2pk(&p.beneficiary), None, bond.utxo.amount - bounty)
                .output(p2pk(&ctx.payer_pub), None, bounty.checked_sub(fee).ok_or("bounty below fee")?),
        )?;
        commit(tx, vec![bond.utxo.clone()])
    })
}

// ---------- invalid variants (built valid, then exactly one thing changed) ----------

fn bad(ctx: &Ctx, b: &TxBuilder<'_>, label: &str, cmd: &str) -> R<()> {
    let forger = load_key("OW_FORGER_KEY", None)?.0;
    let att = load_key("OW_ATTESTER_KEY", None)?.0;
    let p = bond_params(ctx)?;
    let epoch: u64 = env_i("OW_EPOCH", 1)? as u64;
    let (ta, tb) = (std::env::var("OW_TEXT_A").unwrap_or_else(|_| "a".into()), std::env::var("OW_TEXT_B").unwrap_or_else(|_| "b".into()));
    let (ba, bb) = (body_of(&ta), body_of(&tb));
    let (_, sa) = sign_statement(&att, &p.domain, epoch, &ba);
    let (_, sb) = sign_statement(&att, &p.domain, epoch, &bb);
    let valid_slash = || slash(ctx, b, epoch, &ba, &sa, &bb, &sb);
    match cmd {
        "bad-topup" => {
            let ready = topup(ctx, b, p.min_topup as u64)?;
            let mut tx = ready.tx.clone();
            tx.outputs[0].value -= 1;
            tx.outputs[1].value += 1;
            resign_p2pk(&mut tx, &ready.entries, 1, &ctx.payer)?;
            finish_bad(ctx, label, "top-up 1 sompi below min_topup", tx, ready.entries, 0)
        }
        "bad-exit" => {
            let ready = exit_or_rejoin(ctx, b, true, None)?;
            let mut tx = ready.tx.clone();
            resign_actor(&mut tx, &ready.entries, 0, &forger)?;
            resign_p2pk(&mut tx, &ready.entries, 1, &ctx.payer)?;
            finish_bad(ctx, label, "exit signed by a non-attester key", tx, ready.entries, 0)
        }
        "bad-shrink" => {
            let ready = exit_or_rejoin(ctx, b, true, None)?;
            let mut tx = ready.tx.clone();
            tx.outputs[0].value -= 1_000_000;
            tx.outputs[1].value += 1_000_000;
            resign_actor(&mut tx, &ready.entries, 0, &att)?;
            resign_p2pk(&mut tx, &ready.entries, 1, &ctx.payer)?;
            finish_bad(ctx, label, "exit that skims 0.01 tKAS off the bond (attester-signed)", tx, ready.entries, 0)
        }
        "bad-withdraw" => {
            let ready = withdraw(ctx, b, None)?;
            let mut tx = ready.tx.clone();
            resign_actor(&mut tx, &ready.entries, 0, &forger)?;
            finish_bad(ctx, label, "withdraw after the delay signed by a non-attester key", tx, ready.entries, 0)
        }
        "bad-same" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            let script = &mut tx.inputs[0].signature_script;
            replace_once(script, &bb, &ba)?;
            replace_once(script, &sb, &sa)?;
            finish_bad(ctx, label, "slash with the same statement twice", tx, ready.entries, 0)
        }
        "bad-epoch" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            let (_, sb2) = sign_statement(&att, &p.domain, epoch + 1, &bb);
            replace_once(&mut tx.inputs[0].signature_script, &sb, &sb2)?;
            finish_bad(ctx, label, "slash with two statements from different epochs", tx, ready.entries, 0)
        }
        "bad-forger" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            let (_, fb) = sign_statement(&forger, &p.domain, epoch, &bb);
            replace_once(&mut tx.inputs[0].signature_script, &sb, &fb)?;
            finish_bad(ctx, label, "slash where the second statement is signed by someone else", tx, ready.entries, 0)
        }
        "bad-domain" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            let other = blake2b32(std::env::var("OW_OTHER_SEED").unwrap_or_else(|_| "oneword/v1/other".into()).as_bytes());
            let (_, oa) = sign_statement(&att, &other, epoch, &ba);
            let (_, ob) = sign_statement(&att, &other, epoch, &bb);
            let script = &mut tx.inputs[0].signature_script;
            replace_once(script, &sa, &oa)?;
            replace_once(script, &sb, &ob)?;
            finish_bad(ctx, label, "slash with a real equivocation made under another bond's domain", tx, ready.entries, 0)
        }
        "bad-greedy" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            tx.outputs[0].value -= 1;
            tx.outputs[1].value += 1;
            finish_bad(ctx, label, "slasher takes 1 sompi more than the bounty", tx, ready.entries, 0)
        }
        "bad-dest" => {
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            tx.outputs[0].script_public_key = p2pk(&forger.x_only_public_key().0.serialize());
            finish_bad(ctx, label, "beneficiary share paid to another key", tx, ready.entries, 0)
        }
        "bad-twoin" => {
            // Take a builder-made payer input (with its compute commitment) from a valid top-up of the same bond,
            // add it to a valid slash with an equal change output, and re-sign it.
            let donor = topup(ctx, b, p.min_topup as u64)?;
            let ready = valid_slash()?;
            let mut tx = ready.tx.clone();
            let coin = payer_coin(ctx)?;
            tx.inputs.push(donor.tx.inputs[1].clone());
            let extra_fee = 20_000 * ctx.rate;
            tx.outputs.push(kaspa_consensus_core::tx::TransactionOutput::new(coin.entry.amount - extra_fee, p2pk(&ctx.payer_pub)));
            let mut entries = ready.entries.clone();
            entries.push(coin.entry.clone());
            resign_p2pk(&mut tx, &entries, 1, &ctx.payer)?;
            finish_bad(ctx, label, "slash with a second input (could double-count output 0 across two bonds)", tx, entries, 0)
        }
        _ => Err(format!("unknown command {cmd}").into()),
    }
}

fn finish_bad(ctx: &Ctx, label: &str, what: &str, tx: Transaction, entries: Vec<UtxoEntry>, check_input: usize) -> R<()> {
    let ready = commit(tx, entries.clone())?;
    let local = match execute_input_with_covenants(&ready.tx, entries, check_input) {
        Ok(()) => "UNEXPECTED-ACCEPT".to_string(),
        Err(e) => format!("reject: {e}"),
    };
    write_row(ctx, label, what, "reject", &ready, &local, ctx.state.clone())
}

fn resign_actor(tx: &mut Transaction, entries: &[UtxoEntry], index: usize, key: &Keypair) -> R<()> {
    let script = tx.inputs[index].signature_script.clone();
    let mut at = None;
    for k in 0..script.len().saturating_sub(66) {
        if script[k] == 65 && script[k + 65] == SIG_HASH_ALL.to_u8() {
            if at.is_some() {
                return Err("ambiguous signature push".into());
            }
            at = Some(k + 1);
        }
    }
    let at = at.ok_or("signature push not found")?;
    let mtx = MutableTransaction::with_entries(tx.clone(), entries.to_vec());
    let sig = sign(&mtx, index, key);
    tx.inputs[index].signature_script[at..at + 65].copy_from_slice(&sig);
    Ok(())
}

fn resign_p2pk(tx: &mut Transaction, entries: &[UtxoEntry], index: usize, key: &Keypair) -> R<()> {
    let mtx = MutableTransaction::with_entries(tx.clone(), entries.to_vec());
    tx.inputs[index].signature_script = p2pk_signature(&sign(&mtx, index, key));
    Ok(())
}

fn replace_once(hay: &mut [u8], from: &[u8], to: &[u8]) -> R<()> {
    if from.len() != to.len() {
        return Err("widths differ".into());
    }
    let hits: Vec<usize> = (0..=hay.len() - from.len()).filter(|&k| &hay[k..k + from.len()] == from).collect();
    if hits.len() != 1 {
        return Err(format!("pattern found {} times", hits.len()).into());
    }
    hay[hits[0]..hits[0] + to.len()].copy_from_slice(to);
    Ok(())
}

// ---------- selftest ----------

/// Offline run of every path with public demo keys and synthetic coins. No node, no real keys.
fn selftest(art: &Artifact) -> R<()> {
    let dir = std::env::temp_dir().join(format!("oneword-selftest-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let demo = |seed: u8| -> R<(Keypair, [u8; 32], String)> {
        let key = Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&[seed; 32])?);
        let path = dir.join(format!("demo-{seed:02x}.key"));
        fs::write(&path, hex(&[seed; 32]))?;
        Ok((key, key.x_only_public_key().0.serialize(), path.to_string_lossy().into_owned()))
    };
    let (payer, payer_pub, _) = demo(0xc0)?;
    let (_, _, att_path) = demo(0xa1)?;
    let (_, _, forger_path) = demo(0xf1)?;
    unsafe {
        std::env::set_var("OW_ATTESTER_KEY", &att_path);
        std::env::set_var("OW_FORGER_KEY", &forger_path);
    }
    let builder = TxBuilder::new(art)?;
    let mut fund = Vec::new();
    for (n, byte) in [(0u8, 0xf0u8), (1, 0xf2)] {
        let f = dir.join(format!("funding{n}.txt"));
        fs::write(&f, format!("txid={}\nindex=0\namount={}\ncoinbase=0\nblockDaa=0\nscript={}\n",
            TransactionId::from_bytes([byte; 32]), 3 * KAS, hex(p2pk(&payer_pub).script())))?;
        fund.push(f.to_string_lossy().into_owned());
    }
    let mut state = json!({});
    let steps: Vec<(&str, &str, Vec<String>)> = vec![
        ("A", "genesis", vec![fund[0].clone()]),
        ("B", "genesis", vec!["payer".into()]),
        ("C", "genesis", vec![fund[1].clone()]),
        ("A", "bad-topup", vec![]),
        ("A", "topup", vec!["20000000".into()]),
        ("A", "bad-exit", vec![]),
        ("A", "bad-shrink", vec![]),
        ("A", "exit", vec![]),
        ("A", "rejoin", vec![]),
        ("A", "exit", vec![]),
        ("A", "bad-withdraw", vec![]),
        ("A", "withdraw", vec![]),
        ("B", "bad-same", vec![]),
        ("B", "bad-epoch", vec![]),
        ("B", "bad-forger", vec![]),
        ("B", "bad-domain", vec![]),
        ("B", "bad-greedy", vec![]),
        ("B", "bad-dest", vec![]),
        ("B", "bad-twoin", vec![]),
        ("B", "slash", vec!["1".into(), "a".into(), "b".into()]),
        ("C", "exit", vec![]),
        ("C", "slash", vec!["2".into(), "c".into(), "d".into()]),
    ];
    let (mut pass, mut fail) = (0, 0);
    for (n, (bond, cmd, extra)) in steps.into_iter().enumerate() {
        let label = format!("{n:02}-{bond}-{cmd}");
        let mut argv = vec![cmd.to_string()];
        argv.extend(extra);
        let ctx = Ctx { payer, payer_pub, out: dir.to_string_lossy().into_owned(), rate: 100, state: state.clone(), bond: bond.into() };
        let row: Value = match run_cmd(&ctx, &builder, &label, cmd, &argv) {
            Ok(()) => serde_json::from_str(&fs::read_to_string(dir.join(format!("{label}.json")))?)?,
            Err(e) => { println!("FAIL {label}: build error {e}"); fail += 1; continue; }
        };
        let ok = match row["expect"].as_str() {
            Some("accept") => row["local"] == "ok",
            _ => row["local"].as_str().is_some_and(|l| l.starts_with("reject")),
        };
        if ok { pass += 1 } else { fail += 1 }
        println!("{} {label} {}", if ok { "PASS" } else { "FAIL" }, row["local"].as_str().unwrap_or("").chars().take(110).collect::<String>());
        if row["expect"] == "accept" {
            state = row["next"].clone();
        }
    }
    fs::remove_dir_all(&dir)?;
    println!("selftest: {pass} passed, {fail} failed");
    if fail > 0 { Err("selftest failed".into()) } else { Ok(()) }
}

// ---------- state and output ----------

fn with_bond(ctx: &Ctx, tx: &Transaction, index: u32, actor: &str) -> R<Value> {
    let mut next = ctx.state.clone();
    next["bonds"][&ctx.bond]["utxo"] = utxo_json(tx, index);
    next["bonds"][&ctx.bond]["actor"] = json!(actor);
    Ok(next)
}

fn closed(ctx: &Ctx, how: &str) -> R<Value> {
    let mut next = ctx.state.clone();
    next["bonds"][&ctx.bond]["actor"] = json!(how);
    Ok(next)
}

fn push_payout(next: &mut Value, tx: &Transaction, index: u32) -> R<()> {
    let mut paid = next.get("payouts").cloned().unwrap_or_else(|| json!([]));
    paid.as_array_mut().ok_or("payouts")?.push(coin_json(tx, index));
    next["payouts"] = paid;
    Ok(())
}

fn utxo_json(tx: &Transaction, index: u32) -> Value {
    let o = &tx.outputs[index as usize];
    json!({"txid": tx.id().to_string(), "index": index, "value": o.value, "script": hex(o.script_public_key.script())})
}

fn coin_json(tx: &Transaction, index: u32) -> Value {
    json!({"txid": tx.id().to_string(), "index": index, "value": tx.outputs[index as usize].value})
}

/// Valid rows are executed locally (every covenant input) before they are written.
fn emit(ctx: &Ctx, label: &str, what: &str, r: &Ready, next: Value) -> R<()> {
    let mut local = "ok".to_string();
    for (k, e) in r.entries.iter().enumerate() {
        if e.covenant_id.is_some() {
            if let Err(err) = execute_input_with_covenants(&r.tx, r.entries.clone(), k) {
                local = format!("LOCAL-FAIL input {k}: {err}");
            }
        }
    }
    let expect = std::env::var("OW_EXPECT").unwrap_or_else(|_| "accept".into());
    write_row(ctx, label, what, &expect, r, &local, next)
}

fn write_row(ctx: &Ctx, label: &str, what: &str, expect: &str, r: &Ready, local: &str, next: Value) -> R<()> {
    let row = json!({"label": label, "what": what, "expect": expect, "local": local, "fee": r.fee.to_string(),
        "storage": r.storage, "compute": r.compute, "transient": r.transient, "feeRate": ctx.rate,
        "tx": serde_json::from_str::<Value>(&tx_json(&r.tx))?, "next": next});
    fs::write(format!("{}/{label}.json", ctx.out), serde_json::to_string(&row)?)?;
    println!("{label}: {what} expect={expect} local={} fee={} storage={} compute={}",
        local.chars().take(160).collect::<String>(), r.fee, r.storage, r.compute);
    Ok(())
}

// ---------- helpers (shared with feetank's tool) ----------

fn sign(tx: &MutableTransaction<Transaction>, index: usize, key: &Keypair) -> Vec<u8> {
    let digest = calc_schnorr_signature_hash(&tx.as_verifiable(), index, SIG_HASH_ALL, &SigHashReusedValuesUnsync::new());
    let mut w = key.sign_schnorr(Message::from_digest(digest.as_bytes())).as_ref().to_vec();
    w.push(SIG_HASH_ALL.to_u8());
    w
}

fn load_key(var: &str, address_var: Option<&str>) -> R<(Keypair, [u8; 32])> {
    let path = std::env::var(var).map_err(|_| var.to_string())?;
    let secret = hex_decode(fs::read_to_string(&path)?.trim())?;
    let key = Keypair::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&secret)?);
    let public = key.x_only_public_key().0.serialize();
    if let Some(av) = address_var {
        let expect = std::env::var(av).map_err(|_| av.to_string())?;
        let address = Address::try_from(expect.trim())?;
        if address.prefix != Prefix::Testnet || address.version != Version::PubKey || address.payload.as_slice() != public {
            return Err("payer key does not match its testnet address".into());
        }
    }
    Ok((key, public))
}

fn load_funding(path: &str, payer_public: &[u8; 32]) -> R<Coin> {
    let text = fs::read_to_string(path)?;
    let get = |k: &str| -> R<String> {
        text.lines().find_map(|l| l.strip_prefix(&format!("{k}=")).map(|v| v.trim().to_string())).ok_or_else(|| format!("funding {k}").into())
    };
    let script = hex_decode(&get("script")?)?;
    if script.as_slice() != p2pk(payer_public).script() {
        return Err("funding script".into());
    }
    Ok(Coin {
        outpoint: TransactionOutpoint::new(TransactionId::from_str(&get("txid")?)?, get("index")?.parse()?),
        entry: UtxoEntry::new(get("amount")?.parse()?, ScriptPublicKey::from_vec(0, script), get("blockDaa")?.parse()?, get("coinbase")? == "1", None),
    })
}

fn env_i(k: &str, d: i64) -> R<i64> {
    Ok(std::env::var(k).ok().map(|v| v.parse()).transpose()?.unwrap_or(d))
}
fn s<'a>(v: &'a Value, k: &str) -> R<&'a str> {
    v.get(k).and_then(|x| x.as_str()).ok_or_else(|| format!("missing {k}").into())
}
fn u(v: &Value, k: &str) -> R<u64> {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).ok_or_else(|| format!("missing {k}").into())
}
fn i(v: &Value, k: &str) -> R<i64> {
    v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).ok_or_else(|| format!("missing {k}").into())
}
fn arr32(h: &str) -> R<[u8; 32]> {
    let v = hex_decode(h)?;
    v.as_slice().try_into().map_err(|_| "need 32 bytes".into())
}

fn p2pk(public_key: &[u8; 32]) -> ScriptPublicKey {
    let mut script = Vec::with_capacity(34);
    script.push(32);
    script.extend_from_slice(public_key);
    script.push(0xac);
    ScriptPublicKey::from_vec(0, script)
}

fn p2pk_signature(raw: &[u8]) -> Vec<u8> {
    let mut script = vec![raw.len() as u8];
    script.extend_from_slice(raw);
    script
}

fn commit(mut tx: Transaction, entries: Vec<UtxoEntry>) -> R<Ready> {
    let calculator = MassCalculator::new(1, 10, 1_000_000_000_000);
    let populated = MutableTransaction::with_entries(tx.clone(), entries.clone());
    let storage = calculator.calc_contextual_masses(&populated.as_verifiable()).ok_or("storage mass")?.storage_mass;
    if storage > STORAGE_LIMIT {
        return Err(format!("storage mass {storage} over the standard limit").into());
    }
    tx.set_storage_mass(storage);
    tx.finalize();
    let masses = calculator.calc_non_contextual_masses(&tx);
    let input_value: u64 = entries.iter().map(|e| e.amount).sum();
    let output_value: u64 = tx.outputs.iter().map(|o| o.value).sum();
    if input_value < output_value {
        return Err("values".into());
    }
    Ok(Ready { tx, entries, storage, compute: masses.compute_mass, transient: masses.transient_mass, fee: input_value - output_value })
}

fn required_fee(item: &Ready, rate: u64) -> u64 {
    let relay = item.compute.max(item.transient.div_ceil(2));
    item.storage.max(relay).saturating_mul(rate).max(1)
}

fn fit_fee(rate: u64, build: impl Fn(u64) -> R<Ready>) -> R<Ready> {
    let mut fee = rate.saturating_mul(2_000).max(1);
    let mut last = String::from("no fee");
    for _ in 0..10 {
        let ready = build(fee)?;
        let need = required_fee(&ready, rate);
        if ready.fee >= need {
            return Ok(ready);
        }
        last = format!("need {need} had {}", ready.fee);
        // Storage mass moves with the change output, so leave a small margin.
        fee = need + rate * 16;
    }
    Err(last.into())
}

fn tx_json(tx: &Transaction) -> String {
    let raw = borsh::to_vec(tx).expect("borsh");
    format!("{{\"id\":\"{}\",\"borsh\":\"{}\"}}", tx.id(), hex(&raw))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(text: &str) -> R<Vec<u8>> {
    if text.len() % 2 != 0 {
        return Err("hex".into());
    }
    (0..text.len()).step_by(2).map(|k| u8::from_str_radix(&text[k..k + 2], 16).map_err(|e| e.into())).collect()
}
