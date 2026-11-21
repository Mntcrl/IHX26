import Foundation

/// The UI owns the operation lifetime; each ledger or worker effect follows a durable checkpoint.
@MainActor
struct CertificationPipeline {
    let pin: PinnedCircuit
    let ledger: LedgerClient
    let host: ProverHost
    let wallet: WalletIdentity
    let registration: RegistrationJournal
    let store: CaptureStore

    func run(record: CaptureRecord, cancellation: ProvingCancellation,
             onUpdate: (CaptureRecord) -> Void, onCleanupWarning: () -> Void,
             onProgress: @escaping @Sendable (CaptureProgressInfo) -> Void) async throws {
        guard var journal = record.certification, journal.descriptorId == pin.id,
              journal.network == pin.settings.xrpl.networkId,
              journal.accountId == wallet.accountId, journal.deviceTx == registration.registrationTx,
              let keyIdentifier = registration.keyIdentifier else {
            throw FlowError.paused("This capture is pinned to another circuit, wallet or device registration. Restore its original setup to resume.")
        }
        var row = record
        let checkpoint = store.evidenceURL(row)
        var resolvedChallenge: CaptureChallenge?
        var evaluation: CaptureEvaluation?
        var witness: CaptureWitness?

        func advance(_ next: CaptureState, reasons: [String] = []) throws {
            journal.enteredAt[next.rawValue] = Date()
            var updated = row
            updated.certification = journal
            try updated.transition(to: next, reasons: reasons)
            try store.save(updated)
            row = updated
            onUpdate(row)
        }

        func challenge() throws -> CaptureChallenge {
            if let resolvedChallenge { return resolvedChallenge }
            guard let statement = journal.statement, let capture = journal.capture,
                  let inclusion = journal.captureLedger, let selection = journal.selection else {
                throw FlowError.paused("The saved CAPTURE or selection header is incomplete. No new challenge will be chosen.")
            }
            let fixed = try pin.circuit.deriveChallenge(core: statement.core, captureTx: capture.hash,
                                                       captureLedger: inclusion, selectionLedger: selection.value)
            resolvedChallenge = fixed
            return fixed
        }

        func requireProofWindow(_ deadline: UInt32) async throws {
            guard try await ledger.latestIndex() < deadline else { throw GalaxyError.LedgerWindow }
        }

        do {
            while true {
                try Task.checkCancellation()
                switch row.state {
                case .ready, .acquiring:
                    throw FlowError.paused("Waiting for the camera to freeze the shutter window.")
                case .canonicalizing:
                    try advance(.committing)
                case .committing:
                    let input = CaptureParameters(network: journal.network, registrationTx: journal.deviceTx,
                                                  preledger: journal.preledger.value, nonce: journal.nonce,
                                                  claimedTimeMs: row.claimedTimeMs, flags: journal.imageFormat.flags,
                                                  accountId: journal.accountId)
                    journal.statement = try await host.commit(pin: pin, input: input, profile: journal.imageFormat,
                                                              raw: store.url(row), checkpoint: checkpoint)
                    try advance(.attesting)
                case .attesting:
                    guard let statement = journal.statement else { throw incompleteJournal }
                    guard try await ledger.latestIndex() < statement.deadline else { throw GalaxyError.LedgerWindow }
                    journal.assertion = try await GalaxyAppAttest.assertDigest(keyIdentifier, digest: statement.assertionHash)
                    try advance(.submittingManifest)
                case .submittingManifest:
                    guard let statement = journal.statement, let assertion = journal.assertion else { throw incompleteJournal }
                    let memo = try buildCaptureMemo(core: statement.core, assertion: assertion)
                    journal.capture = try await ledger.sign(memo: memo, address: wallet.classicAddress, deadline: statement.deadline)
                    try advance(.waitingManifestValidation)
                case .waitingManifestValidation:
                    guard let capture = journal.capture, let statement = journal.statement else { throw incompleteJournal }
                    let index = try await ledger.confirm(capture, address: wallet.classicAddress)
                    _ = try pin.circuit.selectionLedgerIndex(core: statement.core, captureLedger: index)
                    journal.captureLedger = index
                    try advance(.waitingSelectionLedger)
                case .waitingSelectionLedger:
                    guard let statement = journal.statement, let inclusion = journal.captureLedger else { throw incompleteJournal }
                    let index = try pin.circuit.selectionLedgerIndex(core: statement.core, captureLedger: inclusion)
                    journal.selection = LedgerHeader(try await ledger.waitForHeader(index: index))
                    try advance(.generatingWitness)
                case .generatingWitness:
                    let fixed = try challenge()
                    try await requireProofWindow(fixed.info().proofDeadline)
                    try await host.open(pin)
                    let evaluated = try await host.evaluate(challenge: fixed, checkpoint: checkpoint)
                    evaluation = evaluated
                    witness = try await host.witness(evaluation: evaluated)
                    try advance(.generatingProof)
                case .generatingProof:
                    let fixed = try challenge()
                    try await requireProofWindow(fixed.info().proofDeadline)
                    try await host.open(pin)
                    let readyWitness: CaptureWitness
                    if let witness { readyWitness = witness }
                    else if let evaluation { readyWitness = try await host.witness(evaluation: evaluation) }
                    else {
                        let restored = try await host.evaluate(challenge: fixed, checkpoint: checkpoint)
                        readyWitness = try await host.witness(evaluation: restored)
                    }
                    let proof = try await host.prove(witness: readyWitness, cancellation: cancellation, progress: onProgress)
                    evaluation = nil
                    witness = nil
                    journal.proofBytes = proof.proof
                    journal.witnessSeconds = proof.witnessSeconds
                    journal.proveSeconds = proof.proveSeconds
                    try advance(.localProofVerification)
                case .localProofVerification:
                    guard let proof = journal.proofBytes else { throw incompleteJournal }
                    let fixed = try challenge()
                    try await host.open(pin)
                    journal.proofMemo = JournalMemo(try await host.proofMemo(challenge: fixed, proof: proof))
                    journal.proofBytes = nil
                    try advance(.submittingProof)
                case .submittingProof:
                    guard let memo = journal.proofMemo else { throw incompleteJournal }
                    let fixed = try challenge()
                    journal.proof = try await ledger.sign(memo: memo.value, address: wallet.classicAddress,
                                                          deadline: fixed.info().proofDeadline)
                    try advance(.waitingProofValidation)
                case .waitingProofValidation:
                    guard let proof = journal.proof, let selected = journal.selection else { throw incompleteJournal }
                    let fixed = try challenge()
                    if journal.proofLedger == nil {
                        let index = try await ledger.confirm(proof, address: wallet.classicAddress)
                        guard index > selected.index, index <= fixed.info().proofDeadline else { throw GalaxyError.LedgerWindow }
                        journal.proofLedger = index
                        var updated = row
                        updated.certification = journal
                        try store.save(updated)
                        row = updated
                        onUpdate(row)
                    }
                    try await host.export(challenge: fixed, checkpoint: checkpoint, proofTx: proof.hash,
                                          destination: store.certifiedImageURL(row))
                    try advance(.certified)
                case .certified:
                    do { try store.discardCertifiedEvidence(row) } catch { onCleanupWarning() }
                    return
                case .inconclusive, .failed:
                    return
                }
            }
        } catch let error as FlowError {
            switch error {
            case .inconclusive(let reasons): try advance(.inconclusive, reasons: reasons)
            case .failed(let reason): try advance(.failed, reasons: [reason])
            case .paused: throw error
            }
        } catch let error as GalaxyError {
            switch error {
            case .Cancelled: throw CancellationError()
            case .Inconclusive: try advance(.inconclusive, reasons: ["The challenge-specific policy did not pass."])
            case .Network, .Rpc, .Circuit, .Internal, .Randomness, .EvidenceBundle:
                throw error
            default: try advance(.failed, reasons: [FlowError.message(error)])
            }
        }
    }

    private var incompleteJournal: FlowError {
        .paused("This capture’s protected journal is incomplete. Its signed transaction will not be replaced.")
    }
}
