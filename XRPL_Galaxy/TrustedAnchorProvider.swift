import CryptoKit
import Foundation

nonisolated struct TrustedAnchorEvidence: Codable, Sendable {
    let issuer: String
    let token: Data
    let signature: Data
    let issuedAtMilliseconds: Int64
    let expiresAtMilliseconds: Int64
    // The signed broadcast format makes no secure ranging claim.
}

nonisolated struct TrustedAnchorObservation: Sendable {
    let evidence: TrustedAnchorEvidence
    let observedAt: Date
}

nonisolated protocol TrustedAnchorProvider: Sendable {
    func observations(from start: Date, through end: Date) async -> [TrustedAnchorObservation]
}

nonisolated struct NoTrustedAnchors: TrustedAnchorProvider {
    func observations(from start: Date, through end: Date) async -> [TrustedAnchorObservation] { [] }
}

nonisolated enum SignedAnchorValidator {
    static func verify(_ evidence: TrustedAnchorEvidence, pinnedKey: Curve25519.Signing.PublicKey, observedAt: Date) -> Bool {
        let timestamp = Int64(observedAt.timeIntervalSince1970 * 1000)
        guard evidence.issuedAtMilliseconds >= timestamp - 30_000, evidence.issuedAtMilliseconds <= timestamp,
              evidence.expiresAtMilliseconds >= timestamp, evidence.expiresAtMilliseconds <= evidence.issuedAtMilliseconds + 30_000,
              !evidence.issuer.isEmpty, !evidence.issuer.contains("\0"),
              (16...256).contains(evidence.token.count) else { return false }
        let prefix = ["xrpl-galaxy/trusted-broadcast/v1", evidence.issuer, String(evidence.issuedAtMilliseconds),
                      String(evidence.expiresAtMilliseconds), ""].joined(separator: "\0")
        return pinnedKey.isValidSignature(evidence.signature, for: Data(prefix.utf8) + evidence.token)
    }
}
