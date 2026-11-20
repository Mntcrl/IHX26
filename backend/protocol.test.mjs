import { test } from "node:test";
import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import cbor from "cbor";
import { verifyAssertion } from "node-app-attest";
import { assertChallenge, assertionPayload, checkAttestationCertificateValidity, memoFor, sha256, validatedAnchor, verifyAssertionExtensions } from "./protocol.mjs";

const identity = { teamIdentifier: "ABCDEFGHIJ", bundleIdentifier: "test.galaxy" };
const root = "01".repeat(32);
const proof = assertionPayload("session", "challenge", "nonce", root);
const policy = { allowedValidationCategories: [3], allowedBundleVersions: ["1"], required: true };
const { publicKey, privateKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
const pem = publicKey.export({ type: "spki", format: "pem" });

function fixture(counter, bundle = "1") {
  const header = Buffer.alloc(37);
  sha256(Buffer.from(`${identity.teamIdentifier}.${identity.bundleIdentifier}`)).copy(header);
  header.writeUInt32BE(counter, 33);
  const authenticatorData = Buffer.concat([header, cbor.encode({
    apple_validation_category_01: 3,
    apple_bundle_version_01: bundle,
  })]);
  const signature = sign("sha256", sha256(Buffer.concat([authenticatorData, sha256(proof)])), privateKey);
  return cbor.encode({ signature, authenticatorData });
}

test("challenge is scoped, expiring and single use", () => {
  const row = { purpose: "capture", session: "session", used: 0, expires: 100 };
  assert.doesNotThrow(() => assertChallenge(row, "capture", "session", 99));
  for (const [value, session, now] of [[row, "session", 100], [{ ...row, used: 1 }, "session", 99], [row, "other", 99]]) {
    assert.throws(() => assertChallenge(value, "capture", session, now));
  }
  assert.throws(() => assertChallenge(row, "enroll", "session", 99));
});

test("bound assertion checks signature, challenge, app identity, counter and extension allowlist", () => {
  const assertion = fixture(1);
  const input = { ...identity, assertion, payload: proof, publicKey: pem, signCount: 0 };
  assert.equal(verifyAssertion(input).signCount, 1);
  assert.doesNotThrow(() => verifyAssertionExtensions(assertion, { category: 3, version: "1" }, policy));
  assert.throws(() => verifyAssertion({ ...input, signCount: 1 }));
  assert.throws(() => verifyAssertion({ ...input, payload: assertionPayload("session", "challenge", "changed", root) }));
  assert.throws(() => verifyAssertion({ ...input, bundleIdentifier: "other" }));
  assert.throws(() => verifyAssertion({ ...input, payload: assertionPayload("session", "challenge", "nonce", "ff".repeat(32)) }));
  assert.throws(() => verifyAssertionExtensions(fixture(1, "other"), { category: 3, version: "1" }, policy));
});

test("only validated successful transactions with version and root pass", () => {
  const account = "rTest";
  const tx_json = { Account: account, TransactionType: "AccountSet", Memos: [memoFor(root)] };
  const valid = { validated: true, meta: { TransactionResult: "tesSUCCESS" }, tx_json };
  assert.equal(tx_json.Memos[0].Memo.MemoData.length, 66);
  assert.equal(validatedAnchor(valid, root, account), true);
  assert.equal(validatedAnchor({ ...valid, validated: false }, root, account), false);
  assert.equal(validatedAnchor({ ...valid, meta: { TransactionResult: "tecFAILED" } }, root, account), false);
  assert.equal(validatedAnchor(valid, "ff".repeat(32), account), false);
  assert.equal(validatedAnchor(valid, root, "rOther"), false);
});

test("malformed enrollment certificate chains are rejected", () => {
  assert.throws(() => checkAttestationCertificateValidity(cbor.encode({ fmt: "apple-appattest", attStmt: { x5c: [] } })));
  assert.throws(() => checkAttestationCertificateValidity(cbor.encode({
    fmt: "apple-appattest", attStmt: { x5c: [Buffer.alloc(8), Buffer.alloc(8)] },
  })));
});
