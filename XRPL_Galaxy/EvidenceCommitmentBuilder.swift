import CryptoKit
import Foundation
import Security

nonisolated struct AmbientCaptureEvidence: Codable, Sendable {
    let protocolVersion: Int
    let sessionID: String
    let serverChallenge: CaptureChallenge?
    let claimedCaptureTimestamp: Int64
    let observationWindow: Int
    let observationPreMilliseconds: Int?
    let observationPostMilliseconds: Int?
    let shutterToCommitPreparationMilliseconds: Int?
    let photographHash: String
    let lidarEvidenceHash: String
    let radioFingerprintHash: String
    let sensorAvailability: [String: Bool]
}

nonisolated struct EvidenceCommitment: Codable, Sendable {
    struct Sibling: Codable, Sendable {
        let hash: Data
        let left: Bool
    }

    struct Disclosure: Codable, Sendable {
        let category: String
        let salt: Data
        let fileHashes: [String: String]
        let siblings: [Sibling]
    }

    let protocolVersion: Int
    let root: String
    let disclosures: [Disclosure]
}

nonisolated enum EvidenceCommitmentBuilder {
    static func canonical<T: Encodable>(_ value: T) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return try encoder.encode(value)
    }

    static func fileHash(_ url: URL) throws -> String {
        let input = try FileHandle(forReadingFrom: url)
        defer { try? input.close() }
        var hasher = SHA256()
        while let chunk = try input.read(upToCount: 131_072), !chunk.isEmpty { hasher.update(data: chunk) }
        return hex(hasher.finalize())
    }

    static func hex(_ bytes: some Sequence<UInt8>) -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    static func unhex(_ string: String) -> Data? {
        let bytes = Array(string.utf8)
        guard bytes.count == 64, bytes.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { return nil }
        func nibble(_ byte: UInt8) -> UInt8 { byte <= 57 ? byte - 48 : byte - 87 }
        return Data(stride(from: 0, to: 64, by: 2).map { nibble(bytes[$0]) * 16 + nibble(bytes[$0 + 1]) })
    }

    static func payloadDigest(_ files: [String: String]) throws -> Data {
        guard !files.isEmpty else { throw CocoaError(.fileReadCorruptFile) }
        var payload = Data("xrpl-galaxy/files/v1\0".utf8)
        for (name, hash) in files.sorted(by: { $0.key < $1.key }) {
            guard !name.isEmpty, !name.contains("\0"), !name.contains("/"), !name.contains("\\"),
                  name != ".", name != "..", let bytes = unhex(hash) else { throw CocoaError(.fileReadCorruptFile) }
            payload.append(contentsOf: name.utf8)
            payload.append(0)
            payload.append(bytes)
        }
        return Data(SHA256.hash(data: payload))
    }

    static func leaf(category: String, salt: Data, files: [String: String]) throws -> Data {
        guard ["photo", "lidar", "radio", "metadata", "trustedAnchors"].contains(category),
              salt.count == 32 else { throw CocoaError(.fileReadCorruptFile) }
        let payload = try payloadDigest(files)
        return Data(SHA256.hash(data: Data([0]) + Data(category.utf8) + Data([0]) + salt + payload))
    }

    static func parent(_ left: Data, _ right: Data) -> Data {
        Data(SHA256.hash(data: Data([1]) + left + right))
    }

    static func build(categories: [String: [String: String]]) throws -> EvidenceCommitment {
        let order = ["photo", "lidar", "radio", "metadata", "trustedAnchors"].filter { categories[$0] != nil }
        guard order.count >= 4 else { throw CocoaError(.fileReadCorruptFile) }
        let files = order.map { categories[$0] ?? [:] }
        let salts = try order.map { _ in
            var bytes = Data(count: 32)
            let status = bytes.withUnsafeMutableBytes { buffer in
                SecRandomCopyBytes(kSecRandomDefault, buffer.count, buffer.baseAddress!)
            }
            guard status == errSecSuccess else { throw CocoaError(.fileWriteUnknown) }
            return bytes
        }
        let leaves = try order.indices.map { try leaf(category: order[$0], salt: salts[$0], files: files[$0]) }
        var levels = [leaves]
        var level = leaves
        while level.count > 1 {
            level = stride(from: 0, to: level.count, by: 2).map { parent(level[$0], level[min($0+1, level.count-1)]) }
            levels.append(level)
        }
        let disclosures = order.indices.map { index in
            var cursor = index
            let siblings = levels.dropLast().map { level in
                let odd = cursor % 2 == 1
                let sibling = min(cursor ^ 1, level.count-1)
                cursor /= 2
                return EvidenceCommitment.Sibling(hash: level[sibling], left: odd)
            }
            return EvidenceCommitment.Disclosure(category: order[index], salt: salts[index], fileHashes: files[index], siblings: siblings)
        }
        return EvidenceCommitment(protocolVersion: 1, root: hex(level[0]), disclosures: disclosures)
    }
}

nonisolated enum EvidenceVerifier {
    static func verifies(_ disclosure: EvidenceCommitment.Disclosure, root: String, folder: URL) -> Bool {
        guard let expected = EvidenceCommitmentBuilder.unhex(root), (2...3).contains(disclosure.siblings.count),
              var node = try? EvidenceCommitmentBuilder.leaf(category: disclosure.category, salt: disclosure.salt,
                                                              files: disclosure.fileHashes) else { return false }
        for (name, hash) in disclosure.fileHashes {
            guard (try? EvidenceCommitmentBuilder.fileHash(folder.appendingPathComponent(name))) == hash else { return false }
        }
        for sibling in disclosure.siblings {
            guard sibling.hash.count == 32 else { return false }
            node = sibling.left ? EvidenceCommitmentBuilder.parent(sibling.hash, node) : EvidenceCommitmentBuilder.parent(node, sibling.hash)
        }
        return node == expected
    }

    static func verifiesAll(_ proof: EvidenceCommitment, folder: URL) -> Bool {
        let required: Set<String> = ["photo", "lidar", "radio", "metadata"]
        let supplied = Set(proof.disclosures.map(\.category))
        return proof.protocolVersion == 1 && supplied.count == proof.disclosures.count &&
            required.isSubset(of: supplied) && proof.disclosures.allSatisfy { verifies($0, root: proof.root, folder: folder) }
    }
}
