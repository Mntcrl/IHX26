import Foundation

/// HTTP client for the POV coordinator (plan C8) plus the two XRPL JSON-RPC reads the app needs (C11).
/// Plain `http://<LAN IP>` is fine: ATS does not apply to IP-literal hosts.
nonisolated struct POVClient {
    let base: URL

    func config() async throws -> ConfigResponse {
        try await Self.send(URLRequest(url: base.appendingPathComponent("api/config"), timeoutInterval: 10),
                            as: ConfigResponse.self)
    }

    /// POST /api/capture; the committee answers synchronously after anchoring (or rejecting).
    func capture(_ request: CaptureRequest) async throws -> Attempt {
        var req = try Self.post(base.appendingPathComponent("api/capture"), body: request)
        req.timeoutInterval = 90
        return try await Self.send(req, as: Attempt.self)
    }

    @discardableResult
    func publish(captureId: String, jpeg: Data) async throws -> String {
        let body = PublishRequest(capture_id: captureId, image_b64: jpeg.base64EncodedString())
        var req = try Self.post(base.appendingPathComponent("api/publish"), body: body)
        req.timeoutInterval = 60
        return try await Self.send(req, as: PublishResponse.self).url
    }

    /// Every committee node's on-ledger DID must carry the HPKE key the coordinator advertises,
    /// so a malicious coordinator cannot substitute its own encryption keys.
    static func nodeKeysMatch(_ c: ConfigResponse) async throws -> Bool {
        guard let rpc = URL(string: c.xrpl_rpc) else { throw POVError.notReady("bad xrpl_rpc") }
        for node in c.nodes {
            let body: [String: Any] = ["method": "ledger_entry",
                                       "params": [["did": node.account, "ledger_index": "validated", "api_version": 2]]]
            let entry = try await send(rpcRequest(rpc, body), as: RPCLedgerEntryResponse.self)
            guard let data = entry.result.node?.Data, data.count >= 64,
                  data.prefix(64).lowercased() == node.hpke_pk.lowercased() else { return false }
        }
        return true
    }

    /// Latest validated ledger = T0 (index, hash).
    static func fetchT0(rpc: URL) async throws -> T0 {
        let body: [String: Any] = ["method": "ledger", "params": [["ledger_index": "validated", "api_version": 2]]]
        let ledger = try await send(rpcRequest(rpc, body), as: RPCLedgerResponse.self)
        guard let index = ledger.result.ledger_index, let hash = ledger.result.ledger_hash, hash.count == 64 else {
            throw POVError.notReady(ledger.result.error ?? "ledger unavailable")
        }
        return T0(index: index, hash: hash)
    }

    private static func rpcRequest(_ url: URL, _ body: [String: Any]) throws -> URLRequest {
        var req = URLRequest(url: url, timeoutInterval: 10)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        return req
    }

    private static func post(_ url: URL, body: some Encodable) throws -> URLRequest {
        var req = URLRequest(url: url, timeoutInterval: 20)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONEncoder().encode(body)
        return req
    }

    private static func send<T: Decodable>(_ request: URLRequest, as: T.Type) async throws -> T {
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else { throw POVError.http(0) }
        guard (200..<300).contains(http.statusCode) else { throw POVError.http(http.statusCode) }
        return try JSONDecoder().decode(T.self, from: data)
    }
}
