// Wait until a tx has left the mempool (mined), then until the virtual DAA score has moved past it by <window> + margin.
import { createRequire } from "node:module";
// KASPA_WASM: path to the kaspa-wasm32-sdk nodejs kaspa.js; the "websocket" npm package must be resolvable from this folder.
const require = createRequire(import.meta.url);
globalThis.WebSocket = require("websocket").w3cwebsocket;
const kaspa = require(process.env.KASPA_WASM);
const NODE_URL = process.env.OW_URL; // wRPC Borsh URL of a synced testnet-10 node you run
if (!NODE_URL) { console.log("FAIL set OW_URL"); process.exit(2); }
const [txid, windowArg, marginArg] = process.argv.slice(2);
const want = BigInt(windowArg || "0") + BigInt(marginArg || "60");
const rpc = new kaspa.RpcClient({ url: NODE_URL, encoding: kaspa.Encoding.Borsh, networkId: "testnet-10" });
await rpc.connect({ timeoutDuration: 8000, blockAsyncConnect: true, strategy: "fallback" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const t0 = Date.now();
for (;;) {
  let inMp = false;
  try { inMp = !!(await rpc.getMempoolEntry({ transactionId: txid, includeOrphanPool: true, filterTransactionPool: false })).mempoolEntry; } catch {}
  if (!inMp) break;
  if (Date.now() - t0 > 300000) { console.log("TIMEOUT still in mempool"); process.exit(1); }
  await sleep(1000);
}
const d0 = BigInt((await rpc.getBlockDagInfo()).virtualDaaScore);
for (;;) {
  const d = BigInt((await rpc.getBlockDagInfo()).virtualDaaScore);
  if (d >= d0 + want) { console.log(`left mempool after ${Math.round((Date.now() - t0) / 1000)}s; daa +${d - d0}`); break; }
  await sleep(2000);
}
await rpc.disconnect(); process.exit(0);
