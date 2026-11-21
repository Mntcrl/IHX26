import Foundation
import Security

struct AppAttestBuild: Equatable, Codable, Sendable {
    let appId: String
    let environmentName: String

    var environment: AppAttestEnvironment { environmentName == "production" ? .production : .development }

    static func current(settings: CaptureNetwork) throws -> Self {
        guard let appId = Bundle.main.object(forInfoDictionaryKey: "GalaxyAppID") as? String,
              let environment = Bundle.main.object(forInfoDictionaryKey: "GalaxyAppAttestEnvironment") as? String,
              settings.acceptedAppIds.contains(appId),
              environment == "development" || environment == "production" else {
            throw FlowError.failed("This build’s App Attest identifier or environment does not match the Testnet setup.")
        }
        return Self(appId: appId, environmentName: environment)
    }
}

struct RegistrationJournal: Codable, Sendable {
    let network: UInt32
    let accountId: Data
    let build: AppAttestBuild

    var preparation: JournalTransaction?
    var preparationLedger: UInt32?
    var keyIdentifier: String?
    var challengeLedger: LedgerHeader?
    var nonce: Data?
    var bundle: Data?
    var bundleId: Data?
    var chunks: [JournalMemo] = []
    var deadline: UInt32?
    var chunkTransactions: [JournalTransaction] = []
    var chunkLedgers: [UInt32] = []
    var manifest: JournalTransaction?
    var manifestLedger: UInt32?
    var failure: String?

    var registrationTx: String? { manifestLedger == nil ? nil : manifest?.hash }
    var allTransactions: [JournalTransaction] {
        [preparation].compactMap { $0 } + chunkTransactions + [manifest].compactMap { $0 }
    }
}

func randomBytes(count: Int) throws -> Data {
    guard count > 0 else { throw GalaxyError.InvalidInput }
    var bytes = Data(count: count)
    let status = bytes.withUnsafeMutableBytes { (buffer: UnsafeMutableRawBufferPointer) in
        SecRandomCopyBytes(kSecRandomDefault, count, buffer.baseAddress!)
    }
    guard status == errSecSuccess else { throw FlowError.paused("Secure randomness was unavailable. Try again while the device is unlocked.") }
    return bytes
}

struct RegistrationStore: Sendable {
    private let file: URL

    init() throws {
        file = try Self.root().appending(path: "registration.json")
    }

    static func root() throws -> URL {
        let root = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask,
                                              appropriateFor: nil, create: true)
            .appending(path: "GalaxySetup", directoryHint: .isDirectory)
        try ProtectedFiles.directory(root)
        return root
    }

    func load() throws -> RegistrationJournal? {
        do { return try JSONDecoder().decode(RegistrationJournal.self, from: Data(contentsOf: file)) }
        catch CocoaError.fileReadNoSuchFile { return nil }
    }

    func save(_ journal: RegistrationJournal) throws {
        try ProtectedFiles.replace(JSONEncoder().encode(journal), at: file)
    }
}

/// Invoked only by the controller's single wallet operation, even across app interruption.
@MainActor
struct RegistrationFlow {
    let ledger: LedgerClient
    let wallet: WalletIdentity
    let settings: CaptureNetwork
    let build: AppAttestBuild
    let store: RegistrationStore

    func run(previous: RegistrationJournal?, onUpdate: (RegistrationJournal) -> Void) async throws {
        var journal: RegistrationJournal
        if let previous {
            guard previous.network == settings.xrpl.networkId, previous.accountId == wallet.accountId,
                  previous.build == build else {
                throw FlowError.paused("The existing registration is bound to a different wallet, App Attest build or network.")
            }
            guard previous.registrationTx == nil else { return }
            if previous.failure == nil {
                journal = previous
            } else {
                // Only a definitive failure/expiry may start over. Keep a validated preparation.
                journal = newJournal()
                if previous.preparationLedger != nil {
                    journal.preparation = previous.preparation
                    journal.preparationLedger = previous.preparationLedger
                }
            }
        } else {
            journal = newJournal()
        }
        func persist() throws { try store.save(journal); onUpdate(journal) }

        do {
            if journal.preparationLedger == nil {
                if journal.preparation == nil {
                    // The actual chunk count becomes known after App Attest. At minimum one chunk
                    // and one manifest, plus account preparation, must fit above the reserve.
                    try await requireFunds(for: 3)
                    journal.preparation = try await ledger.signPreparation(address: wallet.classicAddress)
                    try persist()
                }
                guard let signed = journal.preparation else { throw FlowError.paused("The account preparation journal is incomplete.") }
                journal.preparationLedger = try await ledger.confirm(signed, address: wallet.classicAddress)
                try persist()
            }

            if journal.bundle == nil {
                try Task.checkCancellation()
                if journal.keyIdentifier == nil {
                    try await requireFunds(for: 2)
                    journal.keyIdentifier = try await GalaxyAppAttest.generateKeyForRegistration()
                    try persist()
                }
                guard let keyIdentifier = journal.keyIdentifier,
                      let keyId = Data(base64Encoded: keyIdentifier), keyId.count == 32 else {
                    throw FlowError.failed("App Attest returned an invalid key identifier.")
                }
                if journal.challengeLedger == nil {
                    journal.challengeLedger = LedgerHeader(try await ledger.latestHeader())
                    journal.nonce = try randomBytes(count: 32)
                    try persist()
                }
                guard let challengeLedger = journal.challengeLedger, let nonce = journal.nonce else {
                    throw FlowError.paused("The original registration challenge could not be read.")
                }
                let challenge = RegistrationChallenge(network: journal.network, ledger: challengeLedger.value,
                                                      nonce: nonce, keyId: keyId, accountId: journal.accountId)
                let client = try deviceRegistrationClientData(input: challenge)
                guard try await ledger.latestIndex() < client.deadline else { throw GalaxyError.LedgerWindow }
                let attestation = try await GalaxyAppAttest.attestDigest(keyIdentifier, digest: client.clientDataHash)
                let publication = try prepareDevicePublication(input: challenge, appId: build.appId,
                                                               environment: build.environment, attestation: attestation)
                journal.bundle = publication.bundle
                journal.bundleId = publication.bundleId
                journal.chunks = publication.chunks.map(JournalMemo.init)
                journal.deadline = publication.deadline
                try persist()
            }

            guard let bundle = journal.bundle, let deadline = journal.deadline, !journal.chunks.isEmpty,
                  journal.chunkLedgers.count <= journal.chunkTransactions.count,
                  journal.chunkTransactions.count <= journal.chunks.count,
                  journal.chunkTransactions.count - journal.chunkLedgers.count <= 1 else {
                throw FlowError.paused("The original device publication journal could not be read.")
            }
            if journal.chunkTransactions.isEmpty { try await requireFunds(for: journal.chunks.count + 1) }
            for index in journal.chunkLedgers.count..<journal.chunks.count {
                try Task.checkCancellation()
                if index == journal.chunkTransactions.count {
                    // The manifest needs a ledger after the final chunk; don't spend its last slot.
                    let signed = try await ledger.sign(memo: journal.chunks[index].value, address: wallet.classicAddress,
                                                      deadline: deadline - 1)
                    journal.chunkTransactions.append(signed)
                    try persist()
                }
                let indexOnLedger = try await ledger.confirm(journal.chunkTransactions[index], address: wallet.classicAddress)
                journal.chunkLedgers.append(indexOnLedger)
                try persist()
            }
            if journal.manifest == nil {
                let memo = try buildDeviceManifest(bundle: bundle, chunkLedgers: journal.chunkLedgers)
                journal.manifest = try await ledger.sign(memo: memo, address: wallet.classicAddress, deadline: deadline)
                try persist()
            }
            guard let manifest = journal.manifest else { throw FlowError.paused("The signed DEVICE transaction could not be read.") }
            journal.manifestLedger = try await ledger.confirm(manifest, address: wallet.classicAddress)
            try persist()
        } catch let error as FlowError {
            if case .failed(let reason) = error {
                journal.failure = reason
                try persist()
            }
            throw error
        } catch let error as GalaxyError {
            if error == .LedgerWindow || error == .Attestation {
                journal.failure = FlowError.message(error)
                try persist()
            }
            throw error
        }
    }

    private func newJournal() -> RegistrationJournal {
        RegistrationJournal(network: settings.xrpl.networkId, accountId: wallet.accountId, build: build)
    }

    private func requireFunds(for count: Int) async throws {
        let funding = try await ledger.funding(address: wallet.classicAddress)
        guard funding.canSpend(UInt64(count) * settings.xrpl.maxFeeDrops) else {
            throw FlowError.paused("Fund this wallet on Testnet and wait for its validated balance to cover the reserve and the registration fees.")
        }
    }
}
