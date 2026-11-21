import CryptoKit
import Darwin
import Dispatch
import Foundation
import os

struct PinnedCircuit: Sendable {
    let id: String
    let circuit: CaptureCircuit
    let settings: CaptureNetwork
    let limits: CapturePolicyLimits
    let ceremony: String
    let policyID: String
    let policyName: String
    let constraintCount: UInt64

    private struct DescriptorPolicy: Decodable {
        let policyId: String
        let profile: String
        let nConstraints: UInt64
    }

    static func bundled() throws -> Self {
        guard let path = Bundle.main.url(forResource: "descriptor", withExtension: "json") else {
            throw FlowError.failed("The approved circuit descriptor is missing from this build.")
        }
        let descriptor = try Data(contentsOf: path)
        let circuit = try CaptureCircuit.configure(descriptor: descriptor)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let policy = try decoder.decode(DescriptorPolicy.self, from: descriptor)
        var sha = SHA256()
        sha.update(data: Data("GALAXY/CIRCUIT/v1\0".utf8))
        sha.update(data: descriptor)
        let id = sha.finalize().map { String(format: "%02x", $0) }.joined()
        return Self(id: id, circuit: circuit, settings: circuit.network(), limits: circuit.policyLimits(),
                    ceremony: circuit.ceremony(), policyID: policy.policyId, policyName: policy.profile,
                    constraintCount: policy.nConstraints)
    }
}

private struct CommitCheckpoint: Codable {
    let statement: CaptureStatementRecord
    let bundle: Data
}

/// Fixed event names only: never pass evidence, transaction data or errors.
func proofTrace(_ event: String) {
    Logger(subsystem: "utcq.XRPL-Galaxy", category: "CertificationProver")
        .notice("\(event, privacy: .public)")
    #if DEBUG
    print("GXPROVE cert \(Date().timeIntervalSince1970) \(event)")
    #endif
}

private final class ProgressRelay: CaptureProgress {
    let report: @Sendable (CaptureProgressInfo) -> Void

    init(_ report: @escaping @Sendable (CaptureProgressInfo) -> Void) { self.report = report }

    func phaseStarted(phase: String) {
        proofTrace("Rust phase started: \(phase)")
        report(.init(phase: phase, seconds: nil))
    }
    func phaseFinished(phase: String, seconds: Double) {
        proofTrace("Rust phase finished: \(phase), elapsed \(seconds)s")
        report(.init(phase: phase, seconds: seconds))
    }
}

/// Keep blocking UniFFI work off the Swift cooperative pool, at foreground QoS.
actor ProverHost {
    private let worker = DispatchSerialQueue(label: "galaxy.prover", qos: .userInitiated)
    nonisolated var unownedExecutor: UnownedSerialExecutor { worker.asUnownedSerialExecutor() }

    private var prover: CaptureProver?
    private var descriptorId: String?

    func open(_ pin: PinnedCircuit) throws {
        proofTrace("prover actor: open entered")
        defer { proofTrace("prover actor: open exited") }
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
            proofTrace("prover actor: open_pinned including artifact digests")
            let checked = try pin.circuit.openProver(graphPath: folder.appending(path: "main_v1m.graph.bin").path,
                                                    zkeyPath: folder.appending(path: "main_v1m.zkey").path)
            proofTrace("prover actor: open_pinned returned")
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
        proofTrace("prover actor: evaluate entered")
        defer { proofTrace("prover actor: evaluate exited") }
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
        proofTrace("prover actor: build witness entered")
        defer { proofTrace("prover actor: build witness exited") }
        try Task.checkCancellation()
        return try evaluation.buildWitness()
    }

    func prove(witness: CaptureWitness, cancellation: ProvingCancellation,
               progress: @escaping @Sendable (CaptureProgressInfo) -> Void) throws -> CaptureProof {
        proofTrace("prover actor: prove entered")
        try Task.checkCancellation()
        proofTrace("prover actor: call Rust prove")
        defer { proofTrace("prover actor: Rust prove call exited") }
        return try requireProver().prove(witness: witness, progress: ProgressRelay(progress), cancellation: cancellation)
    }

    #if DEBUG
    /// Offline exercise of the same pin, statement, witness, proof and verifier as certification.
    /// No wallet, App Attest, network access, transaction, or certified image is involved.
    func selfTest(cancellation: ProvingCancellation,
                  progress: @escaping @Sendable (CaptureProgressInfo) -> Void) throws -> CaptureProof {
        let documents = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask,
                                                     appropriateFor: nil, create: false)
        let diagnostics = documents.appending(path: "galaxy-proving-diagnostics", directoryHint: .isDirectory)
        try ProtectedFiles.directory(diagnostics)
        let log = try ProvingSelfTestLog(at: diagnostics.appending(path: "\(UUID().uuidString).log"))
        let initial = ContinuousClock.now
        log.record("self_test", "start")
        defer { log.record("self_test", "end", seconds: Self.elapsed(since: initial)) }

        func report(_ tick: CaptureProgressInfo) {
            log.record(tick.phase, tick.seconds == nil ? "start" : "finish", seconds: tick.seconds)
            progress(tick)
        }
        func phase<T>(_ name: String, _ body: () throws -> T) throws -> T {
            try Task.checkCancellation()
            report(.init(phase: name, seconds: nil))
            let start = ContinuousClock.now
            do {
                let result = try body()
                report(.init(phase: name, seconds: Self.elapsed(since: start)))
                return result
            } catch {
                log.record(name, "failed", seconds: Self.elapsed(since: start))
                throw error
            }
        }

        let pin = try phase("load_descriptor", PinnedCircuit.bundled)
        try phase("open_pinned_artifacts") { try open(pin) }

        // Genuine LiDAR calibration controls (64/64 passing development draws).
        let controls = ["199E02DB-9B15-4821-92B5-FEECB15874EE", "B1E6CBE5-E8E6-4CC8-BBEB-185F47C5E660"]
        let fixtures = documents.appending(path: "galaxy-fixtures", directoryHint: .isDirectory)
        let candidates = controls.map { fixtures.appending(path: "\($0).gxrw") }
            .filter { FileManager.default.fileExists(atPath: $0.path) }
        guard !candidates.isEmpty else {
            throw FlowError.failed("This phone does not have either of the private calibration fixtures for this self-test.")
        }

        // Fixed local ledger values, but fresh private salts still change the challenge on
        // each ingest. A policy rejection is not a prover failure; try the second control.
        let input = CaptureParameters(
            network: pin.settings.xrpl.networkId, registrationTx: String(repeating: "A1", count: 32),
            preledger: ValidatedLedger(index: 1_000, hash: Data(repeating: 0xA2, count: 32), closeTime: 1),
            nonce: Data(repeating: 0xA3, count: 32), claimedTimeMs: 1_700_000_000_000,
            flags: CaptureImageFormat.jpeg.flags, accountId: Data(repeating: 0xA4, count: 20))
        var eligible: (CaptureChallenge, CaptureEvaluation)?
        for fixture in candidates {
            let evidence = try phase("ingest_fixture") {
                let bytes = try Data(contentsOf: fixture, options: .mappedIfSafe)
                return try ingestGxrw(raw: bytes, profile: CaptureImageFormat.jpeg.profile, captureState: Data())
            }
            let statement = try phase("build_dev_statement") {
                try pin.circuit.buildCaptureStatement(evidence: evidence, input: input)
            }
            let challenge = try phase("derive_dev_challenge") {
                let inclusion: UInt32 = 1_001
                let selection = try pin.circuit.selectionLedgerIndex(core: statement.core, captureLedger: inclusion)
                return try pin.circuit.deriveChallenge(
                    core: statement.core, captureTx: String(repeating: "A5", count: 32), captureLedger: inclusion,
                    selectionLedger: ValidatedLedger(index: selection, hash: Data(repeating: 0xA6, count: 32), closeTime: 2))
            }
            let evaluation = try phase("evaluate") { try challenge.evaluate(evidence: evidence) }
            if evaluation.verdict().pass {
                eligible = (challenge, evaluation)
                break
            }
            log.record("evaluate", "policy_rejected")
        }
        guard let (challenge, evaluation) = eligible else {
            throw FlowError.inconclusive(["Calibration fixtures did not pass their local challenges. No proof was attempted."])
        }
        let witness = try phase("build_witness_input") { try self.witness(evaluation: evaluation) }
        let proof = try phase("prove") {
            try prove(witness: witness, cancellation: cancellation) { tick in
                log.record(tick.phase, tick.seconds == nil ? "start" : "finish", seconds: tick.seconds)
                progress(tick)
            }
        }
        try phase("verify_local") {
            guard try requireProver().verifyProof(challenge: challenge, proof: proof.proof) else {
                throw GalaxyError.ProofVerification
            }
        }
        log.record("self_test", "verified")
        if log.writeFailed { throw FlowError.failed("The proof verified, but its private diagnostics journal could not be written.") }
        return proof
    }

    private static func elapsed(since start: ContinuousClock.Instant) -> Double {
        let duration = start.duration(to: .now).components
        return Double(duration.seconds) + Double(duration.attoseconds) / 1e18
    }
    #endif

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

#if DEBUG
/// Only timings and process metrics leave the prover. No inputs, IDs or proof bytes.
private final class ProvingSelfTestLog: Sendable {
    private let file: FileHandle
    private let lock = NSLock()
    private let sampler: DispatchSourceTimer
    private let metrics = OSAllocatedUnfairLock(initialState: Metrics())
    private let failed = OSAllocatedUnfairLock(initialState: false)

    private struct Metrics {
        var peakFootprint: UInt64 = 0
        var peakThreads: UInt32 = 0
    }

    var writeFailed: Bool { failed.withLock { $0 } }

    init(at url: URL) throws {
        try ProtectedFiles.create(Data(), at: url)
        file = try FileHandle(forWritingTo: url)
        sampler = DispatchSource.makeTimerSource(queue: .global(qos: .utility))
        sampler.schedule(deadline: .now(), repeating: .milliseconds(50))
        sampler.setEventHandler { [weak self] in
            guard let self else { return }
            _ = self.sample()
        }
        sampler.resume()
    }

    deinit {
        sampler.cancel()
        try? file.close()
    }

    private func sample() -> (footprint: UInt64, kernelPeak: UInt64, threads: UInt32, runPeak: Metrics) {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
        let status = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        let footprint = status == KERN_SUCCESS ? info.phys_footprint : 0
        let kernelPeak = status == KERN_SUCCESS ? UInt64(clamping: info.ledger_phys_footprint_peak) : 0
        var list: thread_act_array_t?
        var threadCount: mach_msg_type_number_t = 0
        if task_threads(mach_task_self_, &list, &threadCount) == KERN_SUCCESS, let list {
            for i in 0..<Int(threadCount) { mach_port_deallocate(mach_task_self_, list[i]) }
            vm_deallocate(mach_task_self_, vm_address_t(UInt(bitPattern: list)),
                          vm_size_t(Int(threadCount) * MemoryLayout<thread_t>.stride))
        }
        let threads = threadCount
        let peak = metrics.withLock { peak in
            peak.peakFootprint = max(peak.peakFootprint, footprint)
            peak.peakThreads = max(peak.peakThreads, threads)
            return peak
        }
        return (footprint, kernelPeak, threads, peak)
    }

    func record(_ phase: String, _ event: String, seconds: Double? = nil) {
        let measure = sample()
        let thermal: String
        switch ProcessInfo.processInfo.thermalState {
        case .nominal: thermal = "nominal"
        case .fair: thermal = "fair"
        case .serious: thermal = "serious"
        case .critical: thermal = "critical"
        @unknown default: thermal = "unknown"
        }
        let duration = seconds.map { String(format: " wall_s=%.6f", $0) } ?? ""
        let line = "GXPROVE self-test \(phase) \(event)\(duration) time=\(Date().timeIntervalSince1970)"
            + " footprint=\(measure.footprint) peak_footprint=\(measure.runPeak.peakFootprint)"
            + " kernel_lifetime_peak=\(measure.kernelPeak) threads=\(measure.threads)"
            + " peak_threads=\(measure.runPeak.peakThreads) thermal=\(thermal)"
        print(line)
        lock.lock()
        defer { lock.unlock() }
        do { try file.write(contentsOf: Data((line + "\n").utf8)) }
        catch { failed.withLock { $0 = true } }
    }
}
#endif
