import Foundation

/// User-visible messages are authored here; never surface RPC bodies, App Attest details or paths.
enum FlowError: Error {
    case paused(String)
    case failed(String)
    case inconclusive([String])

    static func message(_ error: Error) -> String {
        if let error = error as? FlowError {
            switch error {
            case .paused(let text), .failed(let text): return text
            case .inconclusive(let reasons): return reasons.joined(separator: " ")
            }
        }
        if error is WalletSeedStore.Failure {
            return "The wallet could not be read or saved in Keychain. Unlock the device and try again."
        }
        guard let error = error as? GalaxyError else {
            if error is URLError || error is DecodingError { return "The Testnet response was unavailable or invalid. Check the connection and resume." }
            return "The operation could not finish. Check protected storage and try again while the device is unlocked."
        }
        switch error {
        case .Network, .Rpc:
            return "Testnet could not confirm the request. Check connectivity, fees and ledger history, then resume."
        case .LedgerWindow: return "The required validated-ledger window has closed or is invalid."
        case .Circuit: return "The circuit descriptor or proving files do not match the pinned Testnet circuit."
        case .Attestation: return "The App Attest credential or assertion was rejected."
        case .CaptureBinding: return "The capture does not match its pinned ledger statement."
        case .Witness, .Proving: return "On-device witness generation or proving failed."
        case .ProofVerification: return "The generated proof did not verify locally."
        case .Image: return "The certified image could not be packaged."
        case .EvidenceBundle: return "The protected evidence bundle could not be restored."
        case .Transaction: return "GalaxyCore rejected the transaction."
        default: return "GalaxyCore could not complete the operation."
        }
    }
}

struct LedgerHeader: Codable, Sendable {
    let index: UInt32
    let hash: Data
    let closeTime: UInt64

    init(_ header: ValidatedLedger) {
        index = header.index
        hash = header.hash
        closeTime = header.closeTime
    }

    var value: ValidatedLedger { ValidatedLedger(index: index, hash: hash, closeTime: closeTime) }
}

struct JournalMemo: Codable, Sendable {
    let type: String
    let data: Data

    init(_ memo: Memo) { type = memo.memoType; data = memo.data }
    var value: Memo { Memo(memoType: type, data: data) }
}

struct JournalTransaction: Codable, Sendable {
    let purpose: String
    let hash: String
    let blob: Data
    let network: UInt32
    let firstPossibleLedger: UInt32
    let lastLedger: UInt32
    let feeDrops: UInt64

    init(_ signed: SignedTransaction, feeDrops: UInt64) {
        purpose = signed.purpose
        hash = signed.hashHex
        blob = signed.blob
        network = signed.networkId
        firstPossibleLedger = signed.firstPossibleLedger
        lastLedger = signed.lastLedger
        self.feeDrops = feeDrops
    }

    var value: SignedTransaction {
        SignedTransaction(purpose: purpose, hashHex: hash, blob: blob, networkId: network,
                          firstPossibleLedger: firstPossibleLedger, lastLedger: lastLedger)
    }
}

struct FundingStatus: Sendable {
    let balanceDrops: UInt64?
    let reserveDrops: UInt64

    func canSpend(_ drops: UInt64) -> Bool {
        guard let balanceDrops, balanceDrops >= reserveDrops else { return false }
        return balanceDrops - reserveDrops >= drops
    }
}

private final class NoRedirects: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
                    completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

private struct WalletHead: Codable {
    let address: String
    let networkId: UInt32
    let accountTxnId: Data
}

/// Inherited by structured child tasks (including parallel fee/account reads).
enum LedgerRetry {
    @TaskLocal static var report: (@MainActor @Sendable (UUID, Int?) -> Void)?
}

actor LedgerClient {
    private let network: XrplNetwork
    private let endpoints: [URL]
    private var preferredEndpoint = 0
    private var checkedEndpoints: Set<URL> = []
    private let session: URLSession
    private let responseLimit = 1_048_576
    private let headFile: URL
    private static let preparationPurpose = "account-prepare"
    private static let requestSeconds: TimeInterval = 8
    private static let retryDelay: Duration = .milliseconds(300)

    private enum TransportFailure: Error {
        case transientHTTP
    }

    init(configuration: CaptureNetwork) throws {
        guard configuration.xrpl.networkId == 1, !configuration.xrpl.allowMainnet,
              let endpoint = URL(string: configuration.rpcUrl), endpoint.scheme == "https",
              endpoint.host != nil, endpoint.user == nil, endpoint.password == nil, endpoint.fragment == nil else {
            throw FlowError.failed("The configured network is not an HTTPS XRPL Testnet endpoint.")
        }
        self.network = configuration.xrpl
        // Official public Testnet alternatives (xrpl.org/docs/tutorials/public-servers).
        endpoints = [endpoint, URL(string: "https://testnet.xrpl-labs.com/")!,
                     URL(string: "https://testnet.honeycluster.io/")!]
        headFile = try RegistrationStore.root().appending(path: "wallet-head.json")
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = Self.requestSeconds
        config.timeoutIntervalForResource = Self.requestSeconds
        config.waitsForConnectivity = false
        config.urlCache = nil
        config.httpShouldSetCookies = false
        session = URLSession(configuration: config, delegate: NoRedirects(), delegateQueue: nil)
    }

    func latestIndex() async throws -> UInt32 {
        let body = try await rpc(xrplServerInfoRequest())
        return try xrplLatestValidated(network: network, body: body)
    }

    func latestHeader() async throws -> ValidatedLedger {
        let index = try await latestIndex()
        return try await header(index: index)
    }

    func header(index: UInt32) async throws -> ValidatedLedger {
        let body = try await rpc(xrplLedgerRequest(index: index))
        return try xrplParseValidatedLedger(index: index, body: body)
    }

    func waitForHeader(index: UInt32,
                       onValidated: @MainActor @Sendable (UInt32) -> Void) async throws -> ValidatedLedger {
        while true {
            let validated = try await latestIndex()
            await onValidated(validated)
            if validated >= index { return try await header(index: index) }
            try await Task.sleep(for: .seconds(1))
        }
    }

    func funding(address: String) async throws -> FundingStatus {
        let server = try await rpc(xrplServerInfoRequest())
        let validated = try xrplLatestValidated(network: network, body: server)
        let reserves = try JSONDecoder().decode(ServerReserves.self, from: Data(server.utf8))
            .result.info.validated_ledger
        let base = try Self.drops(reserves.reserve_base_xrp)
        let increment = try Self.drops(reserves.reserve_inc_xrp)
        let body = try await rpc(xrplAccountInfoRequest(classicAddress: address, validatedLedger: validated))
        let reply = try JSONDecoder().decode(AccountReply.self, from: Data(body.utf8)).result
        if reply.error == "actNotFound" && reply.status == "error" {
            return FundingStatus(balanceDrops: nil, reserveDrops: base)
        }
        guard reply.status == "success", reply.validated == true, reply.ledger_index == validated,
              let account = reply.account_data, account.Account == address,
              let balance = UInt64(account.Balance) else { throw GalaxyError.Rpc }
        let (extra, overflow) = increment.multipliedReportingOverflow(by: UInt64(account.OwnerCount))
        let (reserve, totalOverflow) = base.addingReportingOverflow(extra)
        guard !overflow, !totalOverflow else { throw GalaxyError.Rpc }
        return FundingStatus(balanceDrops: balance, reserveDrops: reserve)
    }

    func requestFaucet(address: String) async throws {
        guard let url = URL(string: "https://faucet.altnet.rippletest.net/accounts") else { throw GalaxyError.Network }
        let bytes = try JSONEncoder().encode(["destination": address])
        // Do not read the response body; some faucet deployments include a seed in it.
        let (stream, response) = try await session.bytes(for: request(url: url, body: bytes))
        stream.task.cancel()
        try check(response, expected: url)
    }

    /// This only signs. The caller must atomically persist this record before calling confirm.
    func sign(memo: Memo, address: String, deadline: UInt32) async throws -> JournalTransaction {
        try await sign(memo: memo, address: address, expiry: deadline)
    }

    func signPreparation(address: String) async throws -> JournalTransaction {
        try await sign(memo: nil, address: address, expiry: nil)
    }

    private func sign(memo: Memo?, address: String, expiry: UInt32?) async throws -> JournalTransaction {
        let validated = try await latestIndex()
        if let expiry, validated >= expiry { throw GalaxyError.LedgerWindow }
        async let fee = rpc(xrplFeeRequest())
        async let account = rpc(xrplAccountInfoRequest(classicAddress: address, validatedLedger: validated))
        let fields = try await xrplAutofill(network: network, validatedLedger: validated, deadline: expiry,
                                            classicAddress: address, feeBody: fee, accountInfoBody: account)
        try Task.checkCancellation()
        let previous = try loadHead(address: address)
        guard fields.accountTxnId == previous, (memo == nil) == (previous == nil) else {
            throw FlowError.paused("The validated wallet transaction chain differs from the saved chain. No transaction was signed.")
        }
        guard let entropy = try WalletSeedStore.load() else { throw FlowError.paused("Unlock the original wallet before resuming.") }
        let signed: SignedTransaction
        if let memo {
            signed = try xrplSignAccountSet(entropy: entropy, network: network, purpose: memo.memoType, fields: fields, memo: memo)
        } else {
            signed = try xrplSignAccountPreparation(entropy: entropy, network: network, fields: fields)
        }
        return JournalTransaction(signed, feeDrops: fields.feeDrops)
    }

    private func loadHead(address: String) throws -> Data? {
        let head: WalletHead
        do { head = try JSONDecoder().decode(WalletHead.self, from: Data(contentsOf: headFile)) }
        catch CocoaError.fileReadNoSuchFile { return nil }
        guard head.address == address, head.networkId == network.networkId, head.accountTxnId.count == 32 else {
            throw FlowError.paused("The saved transaction chain does not belong to this wallet on Testnet.")
        }
        return head.accountTxnId
    }

    private func storeHead(_ record: SignedTransaction, address: String) throws {
        let prior: Data
        if record.purpose == Self.preparationPurpose {
            // SetFlag 5 creates the AccountTxnID field with the all-zero ledger sentinel.
            prior = Data(repeating: 0, count: 32)
        } else {
            let hex = record.hashHex.utf8
            guard hex.count == 64 else { throw GalaxyError.Transaction }
            var decoded = Data(capacity: 32)
            var i = hex.startIndex
            while i < hex.endIndex {
                let end = hex.index(i, offsetBy: 2)
                guard let byte = UInt8(String(decoding: hex[i..<end], as: UTF8.self), radix: 16) else {
                    throw GalaxyError.Transaction
                }
                decoded.append(byte)
                i = end
            }
            prior = decoded
        }
        let head = WalletHead(address: address, networkId: network.networkId, accountTxnId: prior)
        try ProtectedFiles.replace(JSONEncoder().encode(head), at: headFile)
    }

    /// Always query first, including after an interruption. Submission is never inclusion.
    func confirm(_ stored: JournalTransaction, address: String) async throws -> UInt32 {
        let signed = stored.value
        var submitted = false
        var submitRetries = 0
        let retryID = UUID()
        let report = LedgerRetry.report
        do {
        while true {
            try Task.checkCancellation()
            let latest = try await latestIndex()
            guard signed.firstPossibleLedger > 0, latest >= signed.firstPossibleLedger - 1 else {
                throw FlowError.paused("Testnet is behind the ledger used to sign this transaction. Resume when it catches up.")
            }
            let body = try await rpc(xrplTxRequest(network: network, record: signed, validatedLedger: latest))
            switch try xrplParseTx(network: network, record: signed, validatedLedger: latest, body: body) {
            case .validated(let index, _, let code, let success):
                guard success, code == "tesSUCCESS" else {
                    throw FlowError.failed("The transaction validated without tesSUCCESS. XRPL may have charged its fee.")
                }
                // Validated tec transactions do not advance AccountTxnID.
                if submitRetries > 0 { await report?(retryID, nil) }
                try storeHead(signed, address: address)
                return index
            case .expired:
                throw FlowError.failed("The signed transaction expired without validated inclusion.")
            case .pending:
                if latest < signed.lastLedger && !submitted {
                    do {
                        let response = try await rpc(xrplSubmitRequest(network: network, record: signed, validatedLedger: latest),
                                                     allowReplay: false)
                        _ = try xrplParseSubmit(network: network, record: signed, body: response)
                        if submitRetries > 0 { await report?(retryID, nil) }
                    } catch {
                        try Task.checkCancellation()
                        guard Self.isTransient(error), submitRetries < endpoints.count - 1 else { throw error }
                        submitRetries += 1
                        await report?(retryID, submitRetries)
                        // The server may have received the blob. Fail over and query the
                        // persisted hash before even considering another submit.
                        try await Task.sleep(for: Self.retryDelay)
                        continue
                    }
                    submitted = true
                }
            }
            try await waitForLedger()
        }
        } catch {
            if submitRetries > 0 { await report?(retryID, nil) }
            throw error
        }
    }

    private func waitForLedger() async throws { try await Task.sleep(for: .seconds(3)) }

    private func request(url: URL, body: Data) -> URLRequest {
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData,
                                 timeoutInterval: Self.requestSeconds)
        request.httpMethod = "POST"
        request.httpBody = body
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        return request
    }

    private func check(_ response: URLResponse, expected: URL) throws {
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode),
              http.url == expected else { throw GalaxyError.Rpc }
    }

    private func rpc(_ body: String, allowReplay: Bool = true) async throws -> String {
        let id = UUID()
        let report = LedgerRetry.report
        let originalEndpoint = preferredEndpoint
        let maxAttempts = allowReplay ? endpoints.count : 1
        let payload = Data(body.utf8)
        do {
            for attempt in 0..<maxAttempts {
                try Task.checkCancellation()
                if attempt > 0 {
                    await report?(id, attempt)
                    try await Task.sleep(for: Self.retryDelay)
                }
                let index = (originalEndpoint + attempt) % endpoints.count
                let endpoint = endpoints[index]
                do {
                    let serverInfo = xrplServerInfoRequest()
                    let response: String
                    if body == serverInfo {
                        response = try await send(payload, to: endpoint)
                        _ = try xrplLatestValidated(network: network, body: response)
                        checkedEndpoints.insert(endpoint)
                    } else {
                        if !checkedEndpoints.contains(endpoint) {
                            let info = try await send(Data(serverInfo.utf8), to: endpoint)
                            _ = try xrplLatestValidated(network: network, body: info)
                            checkedEndpoints.insert(endpoint)
                        }
                        response = try await send(payload, to: endpoint)
                    }
                    preferredEndpoint = index
                    if attempt > 0 { await report?(id, nil) }
                    return response
                } catch {
                    try Task.checkCancellation()
                    guard Self.isTransient(error) else { throw error }
                    if !allowReplay { preferredEndpoint = (index + 1) % endpoints.count }
                    if attempt + 1 == maxAttempts { throw error }
                }
            }
            throw GalaxyError.Rpc
        } catch {
            await report?(id, nil)
            if error is TransportFailure && allowReplay { throw GalaxyError.Rpc }
            throw error
        }
    }

    private static func isTransient(_ error: Error) -> Bool {
        if error is TransportFailure { return true }
        switch (error as? URLError)?.code {
        case .timedOut, .cannotFindHost, .cannotConnectToHost, .dnsLookupFailed,
             .networkConnectionLost, .notConnectedToInternet, .resourceUnavailable:
            return true
        default: return false
        }
    }

    private func send(_ body: Data, to endpoint: URL) async throws -> String {
        try Task.checkCancellation()
        let (stream, response) = try await session.bytes(for: request(url: endpoint, body: body))
        defer { stream.task.cancel() }
        if let http = response as? HTTPURLResponse, http.url == endpoint,
           http.statusCode == 429 || (500..<600).contains(http.statusCode) {
            throw TransportFailure.transientHTTP
        }
        try check(response, expected: endpoint)
        guard response.expectedContentLength <= Int64(responseLimit) else { throw GalaxyError.Rpc }
        var bytes = Data()
        if response.expectedContentLength > 0 { bytes.reserveCapacity(Int(response.expectedContentLength)) }
        for try await byte in stream {
            guard bytes.count < responseLimit else { throw GalaxyError.Rpc }
            bytes.append(byte)
        }
        guard let text = String(data: bytes, encoding: .utf8) else { throw GalaxyError.Rpc }
        return text
    }

    private static func drops(_ xrp: Decimal) throws -> UInt64 {
        let value = xrp * 1_000_000
        let drops = NSDecimalNumber(decimal: value).uint64Value
        guard value == Decimal(drops) else { throw GalaxyError.Rpc }
        return drops
    }
}

private struct ServerReserves: Decodable {
    struct Result: Decodable {
        struct Info: Decodable {
            struct Validated: Decodable {
                let reserve_base_xrp: Decimal
                let reserve_inc_xrp: Decimal
            }
            let validated_ledger: Validated
        }
        let info: Info
    }
    let result: Result
}

private struct AccountReply: Decodable {
    struct Result: Decodable {
        struct Account: Decodable {
            let Account: String
            let Balance: String
            let OwnerCount: UInt32
        }
        let status: String
        let error: String?
        let validated: Bool?
        let ledger_index: UInt32?
        let account_data: Account?
    }
    let result: Result
}
