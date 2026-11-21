import Foundation
import Observation

@MainActor
@Observable
final class CertificationController {
    private(set) var pin: PinnedCircuit?
    private(set) var wallet: WalletIdentity?
    private(set) var funding: FundingStatus?
    private(set) var build: AppAttestBuild?
    private(set) var registration: RegistrationJournal?
    private(set) var records: [CaptureRecord] = []
    private(set) var busy: String?
    private(set) var message: String?
    private(set) var artifactMessage: String?
    private(set) var artifactsReady = false
    private(set) var loaded = false
    private(set) var takingPicture = false
    private(set) var progress: CaptureProgressInfo?
    var imageFormat: CaptureImageFormat = .png
    var keepEvidence = false

    @ObservationIgnored private var ledger: LedgerClient?
    @ObservationIgnored private let host = ProverHost()
    @ObservationIgnored private var store: CaptureStore?
    @ObservationIgnored private var registrationStore: RegistrationStore?
    @ObservationIgnored private var operation: Task<Void, Never>?
    @ObservationIgnored private var provingCancellation: ProvingCancellation?
    @ObservationIgnored private var progressToken: UUID?
    @ObservationIgnored private var active = false
    @ObservationIgnored private var pausedByUser = false
    @ObservationIgnored private var resumeOnActivation = false
    @ObservationIgnored private var scheduledResume = false

    var pending: CaptureRecord? { records.first { $0.certification != nil && !$0.state.isTerminal } }
    var lastAttempt: CaptureRecord? { records.first { $0.certification != nil } }
    var hasRegistration: Bool { registration?.registrationTx != nil }
    var idle: Bool { busy == nil && !takingPicture }
    var readyToCapture: Bool { loaded && artifactsReady && wallet != nil && hasRegistration && pending == nil && idle }
    var readyToRegister: Bool {
        guard loaded, let wallet, let build, let pin, idle, pending == nil, !hasRegistration else { return false }
        if let registration {
            guard registration.accountId == wallet.accountId, registration.build == build else { return false }
            // Resume after signing without gating on the current balance; the hash must be resolved first.
            if registration.failure == nil { return true }
        }
        return funding?.canSpend(pin.settings.xrpl.maxFeeDrops * 3) ?? false
    }

    func setActive(_ value: Bool) {
        active = value
        if !value {
            if operation != nil {
                resumeOnActivation = !pausedByUser
                operation?.cancel()
                provingCancellation?.cancel()
            }
            return
        }
        if operation != nil, resumeOnActivation { scheduledResume = true; return }
        if !loaded || (!artifactsReady && artifactMessage == nil) { load() }
        else if resumeOnActivation {
            resumeOnActivation = false
            autoResume()
        }
    }

    func load() {
        perform("Loading setup") { [self] in
            loaded = false
            artifactsReady = false
            resumeOnActivation = false
            let captureStore = try CaptureStore()
            records = try captureStore.recover()
            store = captureStore
            for record in records where record.state == .certified {
                do { try captureStore.discardCertifiedEvidence(record) }
                catch { message = "The certified file is safe, but this phone could not remove its temporary private evidence." }
            }
            let approved = try PinnedCircuit.bundled()
            pin = approved
            build = try AppAttestBuild.current(settings: approved.settings)
            ledger = try LedgerClient(configuration: approved.settings)
            let registrationStore = try RegistrationStore()
            self.registrationStore = registrationStore
            let saved = try registrationStore.load()
            let identity = try WalletSeedStore.load().map { try walletIdentity(entropy: $0) }
            if let saved, saved.accountId != identity?.accountId || saved.build != build || saved.network != approved.settings.xrpl.networkId {
                throw FlowError.paused("The saved device registration is bound to a different wallet, App Attest build or network. Restore that setup; nothing was overwritten.")
            }
            wallet = identity
            registration = saved
            loaded = true
            await openArtifacts(approved)
            try Task.checkCancellation()
            if let address = identity?.classicAddress, let ledger {
                do { funding = try await ledger.funding(address: address) }
                catch is CancellationError { throw CancellationError() }
                catch { message = FlowError.message(error) }
            }
            if !Task.isCancelled && !pausedByUser { scheduledResume = true }
        }
    }

    func createWallet() {
        perform("Creating Testnet wallet") { [self] in
            guard wallet == nil, let ledger, registration == nil else { return }
            let identity: WalletIdentity
            if let entropy = try WalletSeedStore.load() {
                identity = try walletIdentity(entropy: entropy)
            } else {
                let entropy = try randomBytes(count: 16)
                identity = try walletIdentity(entropy: entropy)
                try WalletSeedStore.store(entropy)
            }
            wallet = identity
            funding = try await ledger.funding(address: identity.classicAddress)
        }
    }

    func refreshFunding() {
        perform("Reading validated balance") { [self] in
            guard let ledger, let wallet else { return }
            funding = try await ledger.funding(address: wallet.classicAddress)
        }
    }

    func fundWallet() {
        perform("Requesting Testnet XRP") { [self] in
            guard let ledger, let wallet else { return }
            try await ledger.requestFaucet(address: wallet.classicAddress)
            funding = try await ledger.funding(address: wallet.classicAddress)
            message = "Faucet request accepted. Refresh the validated balance before starting registration."
        }
    }

    func register() {
        guard readyToRegister, let wallet, let pin, let build, let ledger, let registrationStore else { return }
        pausedByUser = false
        perform("Publishing device registration") { [self] in
            try await RegistrationFlow(ledger: ledger, wallet: wallet, settings: pin.settings, build: build, store: registrationStore)
                .run(previous: registration) { registration = $0 }
            funding = try await ledger.funding(address: wallet.classicAddress)
        }
    }

    func shutter(_ acquisition: Acquisition) {
        guard readyToCapture, acquisition.canCapture, let pin, let ledger, let wallet,
              let deviceTx = registration?.registrationTx else { return }
        pausedByUser = false
        let format = imageFormat
        let retain = keepEvidence
        perform("Reading pre-capture ledger") { [self, weak acquisition] in
            let nonce = try randomBytes(count: 32)
            let header = try await ledger.latestHeader()
            try Task.checkCancellation()
            guard active, let acquisition, acquisition.canCapture else {
                throw FlowError.paused("The camera was interrupted before the shutter. No capture transaction was signed.")
            }
            let journal = CertificationJournal(descriptorId: pin.id, deviceTx: deviceTx, network: pin.settings.xrpl.networkId,
                                               accountId: wallet.accountId, preledger: LedgerHeader(header),
                                               imageFormat: format, keepEvidence: retain, nonce: nonce,
                                               enteredAt: [CaptureState.acquiring.rawValue: Date()])
            acquisition.policyLimits = pin.limits
            acquisition.capture(certification: journal)
            guard let row = acquisition.record, row.certification?.nonce == nonce, row.state == .acquiring else {
                throw FlowError.paused("The shutter could not start. Check protected storage and try again.")
            }
            takingPicture = true
            replace(row)
        }
    }

    func cameraStorageFailed(_ row: CaptureRecord) {
        guard row.certification != nil else { return }
        takingPicture = false
        loaded = false
        replace(row)
        message = "The camera’s protected record could not be finalized. Unlock the phone and reload setup to recover it."
    }

    func cameraFinished(_ row: CaptureRecord) {
        guard row.certification != nil else { return }
        replace(row)
        takingPicture = false
        if active, row.state == .canonicalizing { resume() }
        else if !row.state.isTerminal { resumeOnActivation = true }
    }

    func unlockRecovery() {
        guard active, idle else { return }
        load()
    }

    func resume() {
        guard active, loaded, let row = pending, let pin, let ledger, let wallet, let registration, let store, idle else { return }
        pausedByUser = false
        let cancellation = ProvingCancellation()
        let token = UUID()
        progressToken = token
        provingCancellation = cancellation
        perform("Certifying capture") { [self] in
            defer { provingCancellation = nil; progress = nil; progressToken = nil }
            try await CertificationPipeline(pin: pin, ledger: ledger, host: host, wallet: wallet,
                                            registration: registration, store: store)
                .run(record: row, cancellation: cancellation, onUpdate: { replace($0) }, onCleanupWarning: {
                    message = "The certified file is safe, but this phone could not remove its temporary private evidence."
                }, onProgress: { [weak self] tick in
                    Task { @MainActor [weak self] in
                        guard let self, self.progressToken == token else { return }
                        self.progress = tick
                    }
                })
        }
    }

    func pause() {
        guard operation != nil else { return }
        pausedByUser = true
        resumeOnActivation = false
        scheduledResume = false
        operation?.cancel()
        provingCancellation?.cancel()
    }

    func certifiedURL(_ row: CaptureRecord) -> URL? {
        guard row.state == .certified, let store, let url = try? store.certifiedImageURL(row),
              FileManager.default.fileExists(atPath: url.path) else { return nil }
        return url
    }

    private func openArtifacts(_ pin: PinnedCircuit) async {
        artifactsReady = false
        artifactMessage = nil
        do { try await host.open(pin); artifactsReady = true }
        catch is CancellationError { }
        catch { artifactMessage = FlowError.message(error) }
    }

    private func replace(_ row: CaptureRecord) {
        if let index = records.firstIndex(where: { $0.id == row.id }) { records[index] = row }
        else { records.insert(row, at: 0) }
    }

    private func autoResume() {
        guard active, !pausedByUser else { return }
        if pending != nil { resume() }
        else if registration?.failure == nil, registration != nil, !hasRegistration { register() }
    }

    /// There is one operation slot per app/wallet. A cancellation does not release it until
    /// the worker returns, and a nonterminal capture or registration blocks a new capture.
    private func perform(_ label: String, _ body: @escaping @MainActor () async throws -> Void) {
        guard active, operation == nil, !takingPicture else { return }
        busy = label
        message = nil
        operation = Task { [self] in
            do { try await body() }
            catch is CancellationError { message = "Paused. Resume to continue with the same ledger statement and signed transactions." }
            catch {
                if Task.isCancelled { message = "Paused. Resume to continue with the saved operation." }
                else { message = FlowError.message(error) }
            }
            operation = nil
            busy = nil
            if scheduledResume {
                scheduledResume = false
                if !active {
                    resumeOnActivation = !pausedByUser
                } else if !loaded || (!artifactsReady && artifactMessage == nil) {
                    load()
                } else {
                    resumeOnActivation = false
                    autoResume()
                }
            }
        }
    }
}
