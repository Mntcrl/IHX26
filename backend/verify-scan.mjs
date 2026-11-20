import { createHash, timingSafeEqual } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";
import { basename, dirname, join, resolve } from "node:path";
import { Client } from "xrpl";
import { rootPattern, sha256, validatedAnchor } from "./protocol.mjs";

const [path, expectedRoot, transactionHash, account] = process.argv.slice(2);
if (!path || !rootPattern.test(expectedRoot ?? "") || (!!transactionHash !== !!account)) {
  console.error("Usage: node verify-scan.mjs <proof.json or inclusion-proof.json> <trusted-root-hex> [testnet-tx-hash signer-account]");
  process.exit(2);
}
const folder = dirname(resolve(path));
const proof = JSON.parse(await readFile(path, "utf8"));
const categories = ["photo", "lidar", "radio", "metadata", "trustedAnchors"];
const disclosed = new Set();
const assert = (condition, message) => { if (!condition) throw new Error(message); };
assert(proof.protocolVersion === 1 && proof.root === expectedRoot, "Unexpected version or root");
assert(Array.isArray(proof.disclosures) && (proof.disclosures.length === 1 || proof.disclosures.length >= 4), "Invalid disclosure count");
for (const entry of proof.disclosures) {
  assert(categories.includes(entry.category) && !disclosed.has(entry.category), "Invalid category");
  disclosed.add(entry.category);
  const files = Object.entries(entry.fileHashes).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0);
  const salt = Buffer.from(entry.salt, "base64");
  assert(salt.length === 32 && files.length > 0, "Malformed disclosure");
  const parts = [Buffer.from("xrpl-galaxy/files/v1\0")];
  for (const [name, expected] of files) {
    assert(/^[A-Za-z0-9._-]+$/.test(name) && name === basename(name) && name !== "." && name !== ".." &&
           rootPattern.test(expected), "Invalid evidence filename or digest");
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(join(folder, name))) hash.update(chunk);
    assert(hash.digest("hex") === expected, `File integrity failed: ${name}`);
    parts.push(Buffer.from(name), Buffer.from([0]), Buffer.from(expected, "hex"));
  }
  let leaf = sha256(Buffer.concat([Buffer.from([0]), Buffer.from(entry.category), Buffer.from([0]), salt,
                                  sha256(Buffer.concat(parts))]));
  assert([2, 3].includes(entry.siblings?.length), "Malformed inclusion path");
  for (const sibling of entry.siblings) {
    const hash = Buffer.from(sibling.hash, "base64");
    assert(hash.length === 32 && typeof sibling.left === "boolean", "Malformed sibling");
    leaf = sha256(Buffer.concat(sibling.left ? [Buffer.from([1]), hash, leaf] : [Buffer.from([1]), leaf, hash]));
  }
  assert(timingSafeEqual(leaf, Buffer.from(expectedRoot, "hex")), "Merkle inclusion failed");
}
if (proof.disclosures.length > 1) assert(categories.slice(0, 4).every((category) => disclosed.has(category)), "Required proof missing");
console.log(`Evidence integrity verified: ${[...disclosed].join(", ")}`);
if (transactionHash) {
  assert(/^[0-9a-f]{64}$/i.test(transactionHash), "Invalid transaction hash");
  const client = new Client("wss://s.altnet.rippletest.net:51233", { timeout: 12000 });
  await client.connect();
  try {
    const reply = await client.request({ command: "tx", transaction: transactionHash });
    assert(validatedAnchor(reply.result, expectedRoot, account), "Ledger anchoring unverified");
    console.log("XRPL Testnet anchor verified");
  } finally { await client.disconnect(); }
}
