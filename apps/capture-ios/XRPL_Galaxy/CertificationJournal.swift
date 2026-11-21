import Foundation

/// The selected profile controls both ingest and the flags included in the persisted core.
enum CaptureImageFormat: String, CaseIterable, Codable, Identifiable, Sendable {
    case png = "PNG"
    case jpeg = "JPEG"

    var id: Self { self }
    var profile: CaptureProfile { self == .png ? .pngV1 : .jpegV1 }
    var flags: UInt16 { self == .png ? 0 : 1 }
    var fileExtension: String { self == .png ? "png" : "jpg" }
}

struct CertificationJournal: Codable, Sendable {
    let descriptorId: String
    let deviceTx: String
    let network: UInt32
    let accountId: Data
    let preledger: LedgerHeader
    let imageFormat: CaptureImageFormat
    let keepEvidence: Bool
    let nonce: Data
    var accountAddress: String?
    var attestationEnvironment: String?

    var statement: CaptureStatementRecord?
    var assertion: Data?
    var capture: JournalTransaction?
    var captureLedger: UInt32?
    var captureCloseTime: UInt64?
    var selection: LedgerHeader?
    var proofBytes: Data?
    var proofMemo: JournalMemo?
    var proof: JournalTransaction?
    var proofLedger: UInt32?
    var proofValidatedAt: Date?
    var witnessSeconds: Double?
    var proveSeconds: Double?
    var enteredAt: [String: Date] = [:]
}

struct CaptureStatementRecord: Codable, Sendable {
    let core: Data
    let captureId: Data
    let assertionHash: Data
    let deadline: UInt32

    init(_ statement: CaptureStatement) {
        core = statement.core
        captureId = statement.captureId
        assertionHash = statement.assertionClientDataHash
        deadline = statement.captureDeadline
    }
}

extension CaptureStore {
    func evidenceURL(_ record: CaptureRecord) -> URL { directory(record).appending(path: "evidence.gxev") }

    func certifiedImageURL(_ record: CaptureRecord) throws -> URL {
        guard let journal = record.certification else { throw CaptureStore.Failure.invalidRecord }
        return directory(record).appending(path: "Galaxy-\(record.id.uuidString).\(journal.imageFormat.fileExtension)")
    }

    /// Private intermediates produced for this certified attempt only. Never touches fixture exports,
    /// unrelated captures, legacy raw records, or an attempt for which the user opted into retention.
    func discardCertifiedEvidence(_ record: CaptureRecord) throws {
        guard record.state == .certified, record.certification?.keepEvidence == false else { return }
        guard FileManager.default.fileExists(atPath: try certifiedImageURL(record).path) else {
            throw CaptureStore.Failure.invalidRecord
        }
        for file in [url(record), evidenceURL(record)] {
            do { try FileManager.default.removeItem(at: file) }
            catch CocoaError.fileNoSuchFile { continue }
        }
    }
}

struct CaptureProgressInfo: Sendable {
    let phase: String
    let seconds: Double?
}

struct ChallengeReveal: Sendable {
    let ledger: UInt32
    let tiles: [UInt8]
    let startedAt: Date
    let revealedAt: Date
}
