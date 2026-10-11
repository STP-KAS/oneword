// Submit ONE oneword row (built by the oneword tool) to a synced testnet-10 node (OW_URL) and log the result.
// Usage: node submit1.mjs <row.json> <log.jsonl> [replace]
import { createRequire } from "node:module";
import fs from "node:fs";
// KASPA_WASM: path to the kaspa-wasm32-sdk nodejs kaspa.js; the "websocket" npm package must be resolvable from this folder.
const require = createRequire(import.meta.url);
globalThis.WebSocket = require("websocket").w3cwebsocket;
const kaspa = require(process.env.KASPA_WASM);
const NODE_URL = process.env.OW_URL; // wRPC Borsh URL of a synced testnet-10 node you run
if (!NODE_URL) { console.log("FAIL set OW_URL"); process.exit(2); }
const [file, logPath, mode] = process.argv.slice(2);
class Reader {
  constructor(b) { this.b = b; this.pos = 0; }
  u8() { return this.b[this.pos++]; }
  u16() { const v = this.b[this.pos] | (this.b[this.pos + 1] << 8); this.pos += 2; return v; }
  u32() {
    const v = (this.b[this.pos] | (this.b[this.pos + 1] << 8) | (this.b[this.pos + 2] << 16) | (this.b[this.pos + 3] << 24)) >>> 0;
    this.pos += 4;
    return v;
  }
  u64() { return BigInt(this.u32()) + 0x1_0000_0000n * BigInt(this.u32()); }
  bytes(n) { const v = this.b.subarray(this.pos, this.pos + n); this.pos += n; return v; }
  vec() { return this.bytes(this.u32()); }
}
function hex(b) { return Buffer.from(b).toString("hex"); }
function txFromBorsh(hexStr) {
  const bytes = Buffer.from(hexStr, "hex");
  const r = new Reader(bytes);
  const version = r.u16();
  const inputs = Array.from({ length: r.u32() }, () => {
    const transactionId = hex(r.bytes(32));
    const index = r.u32();
    const signatureScript = hex(r.vec());
    const sequence = r.u64();
    const input = { previousOutpoint: { transactionId, index }, signatureScript, sequence, sigOpCount: 0 };
    if (r.u8() === 0) input.sigOpCount = r.u8();
    else input.computeBudget = r.u16();
    return input;
  });
  const outputs = Array.from({ length: r.u32() }, () => {
    const value = r.u64();
    const scriptPublicKey = { version: r.u16(), script: hex(r.vec()) };
    let covenant = null;
    if (r.u8() === 1) covenant = { authorizingInput: r.u16(), covenantId: hex(r.bytes(32)) };
    return { value, scriptPublicKey, covenant };
  });
  const obj = {
    version, inputs, outputs, lockTime: r.u64(), subnetworkId: hex(r.bytes(20)),
    gas: r.u64(), payload: hex(r.vec()), storageMass: r.u64(), id: hex(r.bytes(32)),
  };
  if (r.pos !== bytes.length) throw new Error("borsh length");
  return obj;
}
function binding(covenant) {
  return new kaspa.CovenantBinding(covenant.authorizingInput, new kaspa.Hash(covenant.covenantId));
}
function materialize(obj) {
  const outputs = obj.outputs.map((o) => new kaspa.TransactionOutput(
    o.value,
    new kaspa.ScriptPublicKey(o.scriptPublicKey.version, o.scriptPublicKey.script),
    o.covenant ? binding(o.covenant) : null,
  ));
  const tx = new kaspa.Transaction({
    version: obj.version,
    inputs: obj.inputs,
    outputs,
    lockTime: obj.lockTime,
    subnetworkId: obj.subnetworkId,
    gas: obj.gas,
    payload: obj.payload,
    storageMass: obj.storageMass,
  });
  const live = tx.outputs;
  let patched = false;
  for (let i = 0; i < obj.outputs.length; i++) {
    if (obj.outputs[i].covenant && !live[i].covenant) {
      live[i].covenant = binding(obj.outputs[i].covenant);
      patched = true;
    }
  }
  if (patched) tx.outputs = live;
  return tx;
}
function already(message) {
  return /already (exists|in the mempool|in mempool|accepted|known)|duplicate transaction/i.test(message);
}

const row = JSON.parse(fs.readFileSync(file, "utf8"));
const tx = materialize(txFromBorsh(row.tx.borsh));
if (String(tx.id) !== String(row.tx.id)) { console.log("FAIL id mismatch"); process.exit(2); }
const rpc = new kaspa.RpcClient({ url: NODE_URL, encoding: kaspa.Encoding.Borsh, networkId: "testnet-10" });
await rpc.connect({ timeoutDuration: 8000, blockAsyncConnect: true, strategy: "fallback" });
const si = await rpc.getServerInfo();
if (String(si.networkId) !== "testnet-10" || !si.isSynced) { console.log("FAIL node"); await rpc.disconnect(); process.exit(2); }
const dag = await rpc.getBlockDagInfo();
let ok = false, error = "", replaced = "";
try {
  if (mode === "replace") { const r = await rpc.submitTransactionReplacement({ transaction: tx }); replaced = String(r.replacedTransaction?.id ?? r.replacedTransaction ?? ""); }
  else await rpc.submitTransaction({ transaction: tx, allowOrphan: false });
  ok = true;
} catch (err) { error = String(err.message || err).replace(/\s+/g, " ").slice(0, 400); }
let inMempool = false;
try { inMempool = !!(await rpc.getMempoolEntry({ transactionId: String(tx.id), includeOrphanPool: true, filterTransactionPool: false })).mempoolEntry; } catch {}
const rec = { t: new Date().toISOString(), daa: String(dag.virtualDaaScore), label: row.label, what: row.what, expect: row.expect, local: row.local,
  txid: String(tx.id), how: mode === "replace" ? "replacement" : "submit", ok, fee: row.fee, feeRate: row.feeRate, storage: row.storage, compute: row.compute, inMempoolAfter: inMempool, replaced, error };
fs.appendFileSync(logPath, JSON.stringify(rec) + "\n");
const verdict = (ok ? "ACCEPTED" : "REJECTED") + ((ok ? "accept" : "reject") === row.expect ? " (as expected)" : " (UNEXPECTED)");
console.log(`${row.label}: ${verdict} inMempool=${inMempool}${replaced ? " replaced=" + replaced.slice(0, 8) : ""}${error ? " err=" + error : ""}`);
await rpc.disconnect();
process.exit((ok ? "accept" : "reject") === row.expect ? 0 : 1);
