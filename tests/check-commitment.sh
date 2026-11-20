#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
cat > "$scratch/main.swift" <<'SWIFT'
import Foundation
import CryptoKit

// The protocol value is used by AmbientCaptureEvidence but not by the Merkle test.
struct CaptureChallenge: Codable, Sendable {}
let folder = URL(fileURLWithPath: CommandLine.arguments[1])
let names = ["photo", "lidar", "radio", "metadata", "trustedAnchors"]
var categories: [String: [String: String]] = [:]
for label in names {
    let url = folder.appendingPathComponent(label)
    try Data(label.utf8).write(to: url)
    categories[label] = [label: try EvidenceCommitmentBuilder.fileHash(url)]
}
let proof = try EvidenceCommitmentBuilder.build(categories: categories)
let withoutAnchors = try EvidenceCommitmentBuilder.build(categories: categories.filter { $0.key != "trustedAnchors" })
precondition(EvidenceVerifier.verifiesAll(proof, folder: folder))
precondition(EvidenceVerifier.verifiesAll(withoutAnchors, folder: folder))
precondition(proof.root != (try! EvidenceCommitmentBuilder.build(categories: categories)).root)
let entry = proof.disclosures.first { $0.category == "radio" }!
let selective = EvidenceCommitment(protocolVersion: 1, root: proof.root, disclosures: [entry])
precondition(EvidenceVerifier.verifies(entry, root: proof.root, folder: folder))
try EvidenceCommitmentBuilder.canonical(proof).write(to: folder.appendingPathComponent("proof.json"))
try EvidenceCommitmentBuilder.canonical(selective).write(to: folder.appendingPathComponent("inclusion-proof.json"))
print(proof.root)
SWIFT
swiftc "$root/XRPL_Galaxy/EvidenceCommitmentBuilder.swift" "$scratch/main.swift" -o "$scratch/verify"
proof_root="$("$scratch/verify" "$scratch")"
node "$root/backend/verify-scan.mjs" "$scratch/proof.json" "$proof_root"
node "$root/backend/verify-scan.mjs" "$scratch/inclusion-proof.json" "$proof_root"
printf 'tampered' > "$scratch/radio"
if node "$root/backend/verify-scan.mjs" "$scratch/inclusion-proof.json" "$proof_root" > "$scratch/error" 2>&1; then
    echo "Tampered evidence unexpectedly passed"
    exit 1
fi
grep -q "File integrity failed: radio" "$scratch/error"
echo "Swift and Node agree on the proof, and tampered evidence was rejected."
