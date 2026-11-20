import { createHash, X509Certificate } from "node:crypto";
import cbor from "cbor";

export const sha256 = (bytes) => createHash("sha256").update(bytes).digest();
export const rootPattern = /^[0-9a-f]{64}$/;
export const uuidPattern = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
export const assertionPayload = (session, challenge, nonce, root) =>
  Buffer.from(["xrpl-galaxy/assertion/v1", session, challenge, nonce, root].join("\0"));

export function assertChallenge(row, purpose, session, now = Date.now()) {
  if (!row || row.purpose !== purpose || row.session !== session || row.used !== 0 || row.expires <= now) {
    throw new Error("Invalid, expired or consumed challenge");
  }
}

export function checkAttestationCertificateValidity(bytes, now = Date.now()) {
  const object = cbor.decodeFirstSync(bytes, { preventDuplicateKeys: true, max_depth: 8 });
  const chain = object?.attStmt?.x5c;
  if (object?.fmt !== "apple-appattest" || !Array.isArray(chain) || chain.length !== 2 ||
      !chain.every(Buffer.isBuffer)) throw new Error("Invalid attestation chain");
  const [leaf, intermediate] = chain.map((der) => new X509Certificate(der));
  for (const certificate of [leaf, intermediate]) {
    const from = certificate.validFromDate.valueOf(), to = certificate.validToDate.valueOf();
    if (!Number.isFinite(from) || !Number.isFinite(to) || now < from || now > to) {
      throw new Error("Attestation certificate is not time-valid");
    }
  }
  if (leaf.ca || !intermediate.ca || !leaf.checkIssued(intermediate)) throw new Error("Invalid attestation chain roles");
  // The App Attest verifier separately checks signatures against its pinned Apple root.
}

export function verifyAssertionExtensions(bytes, key, policy) {
  const object = cbor.decodeFirstSync(bytes, { preventDuplicateKeys: true, max_depth: 8 });
  const auth = object.authenticatorData;
  if (!Buffer.isBuffer(auth) || auth.length < 37 || auth.length > 1024) throw new Error("Invalid authenticator data");
  if (auth.length === 37 && !policy.required) return;
  const extension = cbor.decodeFirstSync(auth.subarray(37), { preventDuplicateKeys: true, max_depth: 4 });
  const value = (name) => extension instanceof Map ? extension.get(name) : extension?.[name];
  const category = value("apple_validation_category_01");
  const version = value("apple_bundle_version_01");
  if (!Number.isSafeInteger(category) || typeof version !== "string" ||
      !policy.allowedValidationCategories.includes(category) ||
      !policy.allowedBundleVersions.includes(version) ||
      (key.category != null && key.category !== category) || (key.version != null && key.version !== version)) {
    throw new Error("App Attest extension mismatch");
  }
}

export function memoFor(root) {
  if (!rootPattern.test(root)) throw new Error("Invalid Merkle root");
  return { Memo: { MemoData: `01${root.toUpperCase()}` } };
}

export function validatedAnchor(result, root, account) {
  const tx = result?.tx_json ?? result;
  return result?.validated === true && result.meta?.TransactionResult === "tesSUCCESS" &&
    tx?.Account === account && tx.TransactionType === "AccountSet" && tx.Memos?.length === 1 &&
    Object.keys(tx.Memos[0].Memo ?? {}).length === 1 &&
    tx.Memos[0].Memo?.MemoData?.toUpperCase() === memoFor(root).Memo.MemoData;
}
