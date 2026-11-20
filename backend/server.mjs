import { createServer } from "node:http";
import { randomBytes, randomUUID } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { chmodSync } from "node:fs";
import { verifyAssertion, verifyAttestation } from "node-app-attest";
import { Client, Wallet, hashes } from "xrpl";
import {
  assertChallenge, assertionPayload, checkAttestationCertificateValidity, memoFor, rootPattern, uuidPattern,
  validatedAnchor, verifyAssertionExtensions,
} from "./protocol.mjs";

process.umask(0o077);
const dev = process.env.APP_ATTEST_ENV === "development";
const bundleIdentifier = process.env.APP_BUNDLE_ID ?? "utcq.XRPL-Galaxy";
const teamIdentifier = process.env.APP_ID_PREFIX;
if (!/^[A-Z0-9]{10}$/.test(teamIdentifier ?? "")) throw new Error("Set APP_ID_PREFIX");
const versions = (process.env.APP_BUNDLE_VERSIONS ?? "1").split(",").filter(Boolean);
const policy = {
  allowedValidationCategories: dev ? [3] : [2, 4],
  allowedBundleVersions: versions,
  required: !dev || process.env.APP_ATTEST_REQUIRE_EXTENSIONS === "1",
};
const identity = { bundleIdentifier, teamIdentifier };
const dbPath = process.env.DB_PATH ?? "./evidence.sqlite";
const db = new DatabaseSync(dbPath);
if (dbPath !== ":memory:") chmodSync(dbPath, 0o600);
db.exec(`
  PRAGMA journal_mode = WAL;
  CREATE TABLE IF NOT EXISTS challenges (
    id TEXT PRIMARY KEY, nonce TEXT NOT NULL, purpose TEXT NOT NULL, session TEXT NOT NULL,
    expires INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0
  );
  CREATE TABLE IF NOT EXISTS keys (
    id TEXT PRIMARY KEY, public_key TEXT UNIQUE NOT NULL, counter INTEGER NOT NULL,
    receipt BLOB NOT NULL, category INTEGER, version TEXT, environment TEXT NOT NULL
  );
  CREATE TABLE IF NOT EXISTS commitments (
    root TEXT PRIMARY KEY, created INTEGER NOT NULL, tx_hash TEXT, signed_blob TEXT,
    expires_ledger INTEGER, anchor_status TEXT NOT NULL DEFAULT 'pending', network TEXT, account TEXT
  );
`);
const wallet = process.env.XRPL_TESTNET_SEED ? Wallet.fromSeed(process.env.XRPL_TESTNET_SEED) : null;
// The write path is intentionally restricted to the configured Testnet endpoint.
const endpoint = "wss://s.altnet.rippletest.net:51233";
const network = "testnet";
const getChallenge = db.prepare("SELECT * FROM challenges WHERE id = ?");
const getRoot = db.prepare("SELECT * FROM commitments WHERE root = ?");
const env = dev ? "development" : "production";
const challengeLifetime = 120_000;
const header = { "Content-Type": "application/json", "Cache-Control": "no-store" };

function json(response, status, payload) {
  response.writeHead(status, header);
  response.end(JSON.stringify(payload));
}

async function readJSON(request) {
  if (!request.headers["content-type"]?.startsWith("application/json")) throw new Error("Content type");
  let length = 0;
  const chunks = [];
  for await (const part of request) {
    length += part.length;
    if (length > 40_000) throw new Error("Body too large");
    chunks.push(part);
  }
  const value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid JSON");
  return value;
}

function challengeFor(body, purpose) {
  if (!uuidPattern.test(body.challengeID ?? "") || !uuidPattern.test(body.sessionID ?? "")) throw new Error("Invalid ID");
  const row = getChallenge.get(body.challengeID);
  assertChallenge(row, purpose, body.sessionID);
  return row;
}

function once(challenge, work) {
  db.exec("BEGIN IMMEDIATE");
  try {
    const current = getChallenge.get(challenge.id);
    assertChallenge(current, challenge.purpose, challenge.session);
    const result = work();
    db.prepare("UPDATE challenges SET used = 1 WHERE id = ?").run(current.id);
    db.exec("COMMIT");
    return result;
  } catch (error) {
    db.exec("ROLLBACK");
    throw error;
  }
}

function validKeyID(value) {
  return typeof value === "string" && /^[A-Za-z0-9+/]{43}=$/.test(value);
}

function receipt(row) {
  return {
    root: row.root, applicationAttestationVerified: true, replayProtectionVerified: true,
    xrplAnchoringVerified: row.anchor_status === "validated",
    transactionHash: row.anchor_status === "validated" ? row.tx_hash : null,
    network: row.anchor_status === "validated" ? row.network : null,
  };
}

// One signer worker serializes use of the wallet sequence. Only one process
// should open a writable deployment database and hold this seed.
let working = false;
async function anchorPending() {
  if (!wallet || working) return;
  working = true;
  const client = new Client(endpoint, { timeout: 10_000 });
  try {
    let row = db.prepare("SELECT * FROM commitments WHERE anchor_status = 'pending' ORDER BY signed_blob IS NULL, created LIMIT 1").get();
    if (!row) return;
    if (row.account && (row.account !== wallet.classicAddress || row.network !== network)) throw new Error("Pending transaction uses a different signer");
    await client.connect();
    if (!row.signed_blob) {
      const draft = await client.autofill({ TransactionType: "AccountSet", Account: wallet.classicAddress,
                                           Memos: [memoFor(row.root)] });
      if (!Number.isSafeInteger(draft.LastLedgerSequence) || BigInt(draft.Fee) > 100n) throw new Error("Ledger fee or expiry outside policy");
      const signed = wallet.sign(draft);
      if (hashes.hashSignedTx(signed.tx_blob) !== signed.hash) throw new Error("Signed transaction hash mismatch");
      db.prepare("UPDATE commitments SET tx_hash=?, signed_blob=?, expires_ledger=?, network=?, account=? WHERE root=?")
        .run(signed.hash, signed.tx_blob, draft.LastLedgerSequence, network, wallet.classicAddress, row.root);
      row = getRoot.get(row.root);
    }
    const lookup = async () => {
      try { return (await client.request({ command: "tx", transaction: row.tx_hash })).result; }
      catch (error) { if (error.data?.error === "txnNotFound") return null; throw error; }
    };
    const found = await lookup();
    if (found?.validated) {
      const good = validatedAnchor(found, row.root, wallet.classicAddress);
      db.prepare("UPDATE commitments SET anchor_status = ?, signed_blob = NULL WHERE root = ?")
        .run(good ? "validated" : "failed", row.root);
    } else {
      const ledger = (await client.request({ command: "ledger", ledger_index: "validated", transactions: false })).result;
      if (Number(ledger.ledger_index) > row.expires_ledger) {
        // Absence from a single server is not proof the transaction never landed.
        // Stop and require operator reconciliation before spending again.
        db.prepare("UPDATE commitments SET anchor_status = 'needsReconciliation' WHERE root = ?").run(row.root);
      } else {
        await client.submit(row.signed_blob);
      }
    }
  } catch (error) {
    console.error("Anchor still pending:", error.name);
  } finally {
    if (client.isConnected()) await client.disconnect().catch(() => {});
    working = false;
  }
}
setInterval(() => void anchorPending(), 5_000).unref();

const server = createServer({ requestTimeout: 15_000, headersTimeout: 10_000 }, async (req, res) => {
  try {
    const path = new URL(req.url, "http://localhost").pathname;
    if (req.method === "GET" && path === "/health") return json(res, 200, { ready: true, testnetSignerConfigured: !!wallet });
    if (req.method === "GET" && path.startsWith("/v1/commitments/")) {
      const root = path.split("/").at(-1);
      const row = rootPattern.test(root) ? getRoot.get(root) : null;
      return row ? json(res, 200, receipt(row)) : json(res, 404, { error: "Unknown root" });
    }
    if (req.method !== "POST" || !["/v1/challenges", "/v1/enrollments", "/v1/commitments"].includes(path)) {
      return json(res, 404, { error: "Not found" });
    }
    const body = await readJSON(req);
    if (path === "/v1/challenges") {
      if (!["capture", "enroll"].includes(body.purpose) || !uuidPattern.test(body.sessionID ?? "")) throw new Error("Invalid challenge request");
      const now = Date.now();
      db.prepare("DELETE FROM challenges WHERE expires <= ?").run(now);
      const existing = db.prepare("SELECT COUNT(*) AS count FROM challenges").get().count;
      if (existing >= 2000) return json(res, 429, { error: "Challenge capacity" });
      const id = randomUUID(), nonce = randomBytes(32).toString("base64url"), expiresAtMilliseconds = now + challengeLifetime;
      db.prepare("INSERT INTO challenges(id,nonce,purpose,session,expires) VALUES(?,?,?,?,?)")
        .run(id, nonce, body.purpose, body.sessionID, expiresAtMilliseconds);
      return json(res, 200, { id, nonce, expiresAtMilliseconds });
    }
    if (!validKeyID(body.keyID)) throw new Error("Invalid key");
    if (path === "/v1/enrollments") {
      const challenge = challengeFor(body, "enroll");
      const attestation = Buffer.from(body.attestation, "base64");
      checkAttestationCertificateValidity(attestation);
      const proof = verifyAttestation({ ...identity, attestation, challenge: challenge.nonce, keyId: body.keyID,
                                        allowDevelopmentEnvironment: dev, authenticatorExtensions: policy });
      if (proof.environment !== env) throw new Error("Unexpected App Attest environment");
      once(challenge, () => db.prepare("INSERT INTO keys VALUES (?,?,?,?,?,?,?)")
        .run(body.keyID, proof.publicKey, 0, proof.receipt, proof.validationCategory ?? null, proof.bundleVersion ?? null, env));
      return json(res, 200, { enrolled: true });
    }
    if (body.protocolVersion !== 1 || !rootPattern.test(body.root ?? "")) throw new Error("Invalid root");
    const challenge = challengeFor(body, "capture");
    const assertion = Buffer.from(body.assertion, "base64");
    once(challenge, () => {
      const key = db.prepare("SELECT * FROM keys WHERE id = ? AND environment = ?").get(body.keyID, env);
      if (!key) throw new Error("Unknown key");
      const payload = assertionPayload(body.sessionID, challenge.id, challenge.nonce, body.root);
      const checked = verifyAssertion({ ...identity, assertion, payload, publicKey: key.public_key, signCount: key.counter });
      verifyAssertionExtensions(assertion, key, policy);
      db.prepare("UPDATE keys SET counter = ? WHERE id = ?").run(checked.signCount, body.keyID);
      db.prepare("INSERT OR IGNORE INTO commitments(root,created) VALUES(?,?)").run(body.root, Date.now());
    });
    void anchorPending();
    return json(res, 202, receipt(getRoot.get(body.root)));
  } catch {
    if (!res.headersSent && !res.destroyed) json(res, 400, { error: "Request rejected" });
  }
});
server.listen(Number(process.env.PORT ?? 8787), "127.0.0.1");
console.log("Evidence backend listening on loopback. Use a rate-limited TLS reverse proxy.");
