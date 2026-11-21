import CryptoKit
import Foundation

struct PinnedCircuit: Sendable {
    let id: String
    let circuit: CaptureCircuit
    let settings: CaptureNetwork
    let limits: CapturePolicyLimits
    let ceremony: String

    static func bundled() throws -> Self {
        guard let path = Bundle.main.url(forResource: "descriptor", withExtension: "json") else {
            throw FlowError.failed("The approved circuit descriptor is missing from this build.")
        }
        let descriptor = try Data(contentsOf: path)
        let circuit = try CaptureCircuit.configure(descriptor: descriptor)
        var sha = SHA256()
        sha.update(data: Data("GALAXY/CIRCUIT/v1\0".utf8))
        sha.update(data: descriptor)
        let id = sha.finalize().map { String(format: "%02x", $0) }.joined()
        return Self(id: id, circuit: circuit, settings: circuit.network(), limits: circuit.policyLimits(),
                    ceremony: circuit.ceremony())
    }
}

private struct CommitCheckpoint: Codable {
    let statement: CaptureStatementRecord
    let bundle: Data
}

private final class ProgressRelay: CaptureProgress {
    let report: @Sendable (CaptureProgressInfo) -> Void

    init(_ report: @escaping @Sendable (CaptureProgressInfo) -> Void) { self.report = report }

    func phaseStarted(phase: String) { report(.init(phase: phase, seconds: nil)) }
    func phaseFinished(phase: String, seconds: Double) { report(.init(phase: phase, seconds: seconds)) }
}

/// Dedicated serial executor (not MainActor) for all expensive synchronous UniFFI work.
actor ProverHost {
    private var prover: CaptureProver?
    private var descriptorId: String?

    func open(_ pin: PinnedCircuit) throws {
        if descriptorId == pin.id, prover != nil { return }
        let root = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                              appropriateFor: nil, create: true)
            .appending(path: "galaxy-artifacts", directoryHint: .isDirectory)
        try ProtectedFiles.directory(root)
        try ProtectedFiles.excludeFromBackup(root)
        let circuitDir = root.appending(path: pin.id, directoryHint: .isDirectory)
        try ProtectedFiles.directory(circuitDir)
        let installations = try FileManager.default.contentsOfDirectory(at: circuitDir, includingPropertiesForKeys: nil)
        let marked = installations.sorted { $0.lastPathComponent > $1.lastPathComponent }.first { folder in
            FileManager.default.fileExists(atPath: folder.appending(path: "READY").path)
        }
        guard let folder = marked,
              let marker = try? String(contentsOf: folder.appending(path: "READY"), encoding: .utf8),
              marker == pin.id + "\n" else {
            throw FlowError.paused("Proving files are missing or incomplete. Install the matching artifact pair and reload setup.")
        }
        do {
            // Rust verifies both SHA-256 digests, the VK digest and the public input count before use.
            let checked = try pin.circuit.openProver(graphPath: folder.appending(path: "main_v1m.graph.bin").path,
                                                    zkeyPath: folder.appending(path: "main_v1m.zkey").path)
            try Task.checkCancellation()
            prover = checked
            descriptorId = pin.id
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw FlowError.paused("Proving files are missing or do not match the pinned circuit. Install the approved pair and reload setup.")
        }
    }

    func commit(pin: PinnedCircuit, input: CaptureParameters, profile: CaptureImageFormat,
                raw: URL, checkpoint: URL) throws -> CaptureStatementRecord {
        if FileManager.default.fileExists(atPath: checkpoint.path) { return try restore(checkpoint).statement }
        try Task.checkCancellation()
        let bytes = try Data(contentsOf: raw, options: .mappedIfSafe)
        let evidence: CaptureEvidence
        do { evidence = try ingestGxrw(raw: bytes, profile: profile.profile, captureState: Data()) }
        catch GalaxyError.Evidence { throw FlowError.inconclusive(["The synchronized sensor evidence could not be canonicalized."]) }
        try Task.checkCancellation()
        let statement = CaptureStatementRecord(try pin.circuit.buildCaptureStatement(evidence: evidence, input: input))
        let frozen = CommitCheckpoint(statement: statement, bundle: try evidence.encodeBundle())
        let encoder = PropertyListEncoder()
        encoder.outputFormat = .binary
        try ProtectedFiles.replace(encoder.encode(frozen), at: checkpoint)
        return statement
    }

    func evaluate(challenge: CaptureChallenge, checkpoint: URL) throws -> CaptureEvaluation {
        let evaluation = try challenge.evaluate(evidence: loadEvidence(checkpoint))
        let verdict = evaluation.verdict()
        guard verdict.pass else {
            var reasons: [String] = []
            if !verdict.qualityOk { reasons.append("Capture quality predicate failed.") }
            if !verdict.geometryOk { reasons.append("LiDAR geometry predicate failed.") }
            if !verdict.rgbDepthOk { reasons.append("RGB–depth consistency predicate failed.") }
            if !verdict.temporalOk { reasons.append("Temporal consistency predicate failed.") }
            if !verdict.motionOk { reasons.append("Camera motion predicate failed.") }
            if reasons.isEmpty { reasons.append("The challenge-specific policy did not pass.") }
            throw FlowError.inconclusive(reasons)
        }
        return evaluation
    }

    func witness(evaluation: CaptureEvaluation) throws -> CaptureWitness {
        try Task.checkCancellation()
        return try evaluation.buildWitness()
    }

    func prove(witness: CaptureWitness, cancellation: ProvingCancellation,
               progress: @escaping @Sendable (CaptureProgressInfo) -> Void) throws -> CaptureProof {
        try Task.checkCancellation()
        return try requireProver().prove(witness: witness, progress: ProgressRelay(progress), cancellation: cancellation)
    }

    func proofMemo(challenge: CaptureChallenge, proof: Data) throws -> Memo {
        try Task.checkCancellation()
        // buildProofMemo verifies the proof against this challenge before constructing the memo.
        return try requireProver().buildProofMemo(challenge: challenge, proof: proof)
    }

    func export(challenge: CaptureChallenge, checkpoint: URL, proofTx: String, destination: URL) throws {
        let bytes = try challenge.exportImage(evidence: loadEvidence(checkpoint), proofTx: proofTx)
        try ProtectedFiles.replace(bytes, at: destination)
    }

    private func restore(_ checkpoint: URL) throws -> CommitCheckpoint {
        try PropertyListDecoder().decode(CommitCheckpoint.self, from: Data(contentsOf: checkpoint, options: .mappedIfSafe))
    }

    private func loadEvidence(_ checkpoint: URL) throws -> CaptureEvidence {
        try Task.checkCancellation()
        return try decodeEvidenceBundle(bytes: restore(checkpoint).bundle)
    }

    private func requireProver() throws -> CaptureProver {
        guard let prover else { throw FlowError.paused("Install and open the pinned proving files before continuing.") }
        return prover
    }
}
