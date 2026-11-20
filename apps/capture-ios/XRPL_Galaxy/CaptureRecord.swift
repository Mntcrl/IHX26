import Foundation

nonisolated enum CaptureState: String, Codable, Sendable {
    case ready = "READY"
    case acquiring = "ACQUIRING"
    case canonicalizing = "CANONICALIZING"
    case committing = "COMMITTING"
    case attesting = "ATTESTING"
    case submittingManifest = "SUBMITTING_MANIFEST"
    case waitingManifestValidation = "WAITING_MANIFEST_VALIDATION"
    case waitingSelectionLedger = "WAITING_SELECTION_LEDGER"
    case generatingWitness = "GENERATING_WITNESS"
    case generatingProof = "GENERATING_PROOF"
    case localProofVerification = "LOCAL_PROOF_VERIFICATION"
    case submittingProof = "SUBMITTING_PROOF"
    case waitingProofValidation = "WAITING_PROOF_VALIDATION"
    case certified = "CERTIFIED"
    case inconclusive = "INCONCLUSIVE"
    case failed = "FAILED"

    func permits(_ next: Self) -> Bool {
        switch (self, next) {
        case (.certified, _), (.inconclusive, _), (.failed, _): false
        case (_, .inconclusive), (_, .failed): true
        case (.ready, .acquiring), (.acquiring, .canonicalizing), (.canonicalizing, .committing),
             (.committing, .attesting), (.attesting, .submittingManifest),
             (.submittingManifest, .waitingManifestValidation), (.waitingManifestValidation, .waitingSelectionLedger),
             (.waitingSelectionLedger, .generatingWitness), (.generatingWitness, .generatingProof),
             (.generatingProof, .localProofVerification), (.localProofVerification, .submittingProof),
             (.submittingProof, .waitingProofValidation), (.waitingProofValidation, .certified): true
        default: false
        }
    }
}

nonisolated struct CaptureRecord: Codable, Identifiable, Sendable {
    let id: UUID
    let claimedTimeMs: UInt64
    private(set) var state: CaptureState = .ready
    private(set) var reasons: [String] = []

    mutating func prepareAcquisition(reasons: [String]) throws {
        guard state == .acquiring else { throw CaptureStore.Failure.invalidTransition }
        self.reasons = reasons
    }

    mutating func transition(to next: CaptureState, reasons: [String] = []) throws {
        guard state.permits(next), (next != .failed && next != .inconclusive) || !reasons.isEmpty else {
            throw CaptureStore.Failure.invalidTransition
        }
        state = next
        self.reasons = reasons
    }
}

nonisolated struct CaptureStore: Sendable {
    enum Failure: LocalizedError {
        case invalidTransition, invalidRecord, noRawCapture, alreadyExported

        var errorDescription: String? {
            switch self {
            case .invalidTransition: "Invalid capture state transition."
            case .invalidRecord: "Invalid capture record."
            case .noRawCapture: "There is no complete raw capture available to export."
            case .alreadyExported: "This raw capture was already exported to Documents/galaxy-fixtures."
            }
        }
    }

    let root: URL

    init() throws {
        root = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                           appropriateFor: nil, create: true)
            .appending(path: "GalaxyCaptures", directoryHint: .isDirectory)
        try ProtectedFiles.directory(root)
    }

    func directory(_ record: CaptureRecord) -> URL {
        root.appending(path: record.id.uuidString, directoryHint: .isDirectory)
    }

    func url(_ record: CaptureRecord) -> URL { directory(record).appending(path: "capture.gxrw") }

    func save(_ record: CaptureRecord) throws {
        try ProtectedFiles.directory(directory(record))
        // record.json lives in this app's own per-capture UUID directory; replacing it is allowed.
        try ProtectedFiles.replace(JSONEncoder().encode(record), at: directory(record).appending(path: "record.json"))
    }

    func records() throws -> [CaptureRecord] {
        let folders = try FileManager.default.contentsOfDirectory(at: root, includingPropertiesForKeys: nil)
        return try folders.compactMap { folder in
            guard let id = UUID(uuidString: folder.lastPathComponent) else { return nil }
            let file = folder.appending(path: "record.json")
            // A folder created just before an app kill may not have its first record yet.
            guard FileManager.default.fileExists(atPath: file.path) else { return nil }
            let record = try JSONDecoder().decode(CaptureRecord.self, from: Data(contentsOf: file))
            guard id == record.id else { throw Failure.invalidRecord }
            return record
        }.sorted { ($0.claimedTimeMs, $0.id.uuidString) > ($1.claimedTimeMs, $1.id.uuidString) }
    }

    func recover() throws -> [CaptureRecord] {
        try records().map { record in
            guard record.state == .acquiring else { return record }
            // The acquisition decision is persisted before the raw file is atomically published.
            var next = record
            if hasRaw(record) {
                try next.transition(to: record.reasons.isEmpty ? .canonicalizing : .inconclusive, reasons: record.reasons)
            } else {
                try next.transition(to: .inconclusive, reasons: ["Acquisition was interrupted before the raw capture was frozen."])
            }
            try save(next)
            return next
        }
    }

    func hasRaw(_ record: CaptureRecord) -> Bool { FileManager.default.fileExists(atPath: url(record).path) }

    func export(_ record: CaptureRecord) throws {
        guard hasRaw(record) else { throw Failure.noRawCapture }
        let dir = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                             appropriateFor: nil, create: true)
            .appending(path: "galaxy-fixtures", directoryHint: .isDirectory)
        try ProtectedFiles.directory(dir)
        // The source and destination are local protected files; no share sheet or cloud export.
        // An earlier export of the same capture is never overwritten.
        let destination = dir.appending(path: "\(record.id.uuidString).gxrw")
        guard !FileManager.default.fileExists(atPath: destination.path) else { throw Failure.alreadyExported }
        try ProtectedFiles.create(Data(contentsOf: url(record), options: .mappedIfSafe), at: destination)
    }
}
