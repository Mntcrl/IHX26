import CryptoKit
import DeviceCheck
import Foundation

nonisolated struct CaptureChallenge: Codable, Sendable {
    let id: String
    let nonce: String
    let expiresAtMilliseconds: Int64
}

nonisolated struct AnchorReceipt: Codable, Sendable {
    let root: String
    let applicationAttestationVerified: Bool
    let replayProtectionVerified: Bool
    let xrplAnchoringVerified: Bool
    let transactionHash: String?
    let network: String?
}

nonisolated private final class RefuseRedirects: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

nonisolated struct XRPLAnchorService: Sendable {
    private static let client = URLSession(configuration: .ephemeral, delegate: RefuseRedirects(), delegateQueue: nil)
    let baseURL: URL
    static var configured: Self? {
        guard let value = Bundle.main.object(forInfoDictionaryKey: "AmbientBackendURL") as? String,
              let url = URL(string: value), url.scheme == "https", url.host != nil,
              url.user == nil, url.password == nil, url.query == nil, url.fragment == nil else { return nil }
        return Self(baseURL: url)
    }

    func post<Input: Encodable, Output: Decodable>(_ path: String, _ input: Input, as: Output.Type) async throws -> Output {
        var request = URLRequest(url: baseURL.appendingPathComponent(path), timeoutInterval: 15)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(input)
        return try await fetch(request, as: Output.self)
    }

    func status(root: String) async throws -> AnchorReceipt {
        guard EvidenceCommitmentBuilder.unhex(root) != nil else { throw URLError(.badURL) }
        let request = URLRequest(url: baseURL.appendingPathComponent("v1/commitments/\(root)"), timeoutInterval: 12)
        let receipt = try await fetch(request, as: AnchorReceipt.self)
        guard receipt.root == root else { throw URLError(.badServerResponse) }
        return receipt
    }

    private func fetch<Output: Decodable>(_ request: URLRequest, as: Output.Type) async throws -> Output {
        let (data, response) = try await Self.client.data(for: request)
        guard let response = response as? HTTPURLResponse, response.url?.scheme == "https",
              response.url?.host == baseURL.host, (200..<300).contains(response.statusCode) else {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode(Output.self, from: data)
    }
}

actor CaptureAttestationService {
    private struct ChallengeRequest: Encodable { let purpose: String; let sessionID: String }
    private struct Enrollment: Encodable {
        let challengeID: String
        let sessionID: String
        let keyID: String
        let attestation: Data
    }
    private struct EnrollmentReply: Decodable { let enrolled: Bool }
    private struct AssertionRequest: Encodable {
        let protocolVersion: Int
        let sessionID: String
        let challengeID: String
        let keyID: String
        let root: String
        let assertion: Data
    }

    let backend: XRPLAnchorService
    private var keyID: String?
    private var activeSubmission: (id: UUID, task: Task<AnchorReceipt, Error>)?
    init(backend: XRPLAnchorService) { self.backend = backend }

    func challenge(sessionID: String) async throws -> CaptureChallenge {
        _ = try await prepareKey()
        return try await backend.post("v1/challenges", ChallengeRequest(purpose: "capture", sessionID: sessionID),
                                      as: CaptureChallenge.self)
    }

    func submit(root: String, sessionID: String, challenge: CaptureChallenge) async throws -> AnchorReceipt {
        let prior = activeSubmission?.task
        let id = UUID()
        let task = Task {
            if let prior { _ = try? await prior.value }
            return try await self.send(root: root, sessionID: sessionID, challenge: challenge)
        }
        activeSubmission = (id, task)
        defer { if activeSubmission?.id == id { activeSubmission = nil } }
        return try await task.value
    }

    private func send(root: String, sessionID: String, challenge: CaptureChallenge) async throws -> AnchorReceipt {
        guard challenge.expiresAtMilliseconds > Int64(Date().timeIntervalSince1970 * 1000),
              let keyID else { throw URLError(.userAuthenticationRequired) }
        let message = Self.assertionPayload(sessionID: sessionID, challenge: challenge, root: root)
        let assertion = try await DCAppAttestService.shared.generateAssertion(keyID, clientDataHash: Data(SHA256.hash(data: message)))
        let body = AssertionRequest(protocolVersion: 1, sessionID: sessionID, challengeID: challenge.id, keyID: keyID,
                                    root: root, assertion: assertion)
        let receipt = try await backend.post("v1/commitments", body, as: AnchorReceipt.self)
        guard receipt.root == root else { throw URLError(.badServerResponse) }
        return receipt
    }

    static func assertionPayload(sessionID: String, challenge: CaptureChallenge, root: String) -> Data {
        Data(["xrpl-galaxy/assertion/v1", sessionID, challenge.id, challenge.nonce, root].joined(separator: "\0").utf8)
    }

    private func prepareKey() async throws -> String {
        let service = DCAppAttestService.shared
        guard service.isSupported else { throw URLError(.unsupportedURL) }
        if let keyID { return keyID }
        let storageKey = "appAttestKey:" + backend.baseURL.absoluteString
        if let existing = UserDefaults.standard.string(forKey: storageKey) { keyID = existing; return existing }
        let session = UUID().uuidString.lowercased()
        let challenge = try await backend.post("v1/challenges", ChallengeRequest(purpose: "enroll", sessionID: session),
                                               as: CaptureChallenge.self)
        let key = try await service.generateKey()
        let attest = try await service.attestKey(key, clientDataHash: Data(SHA256.hash(data: Data(challenge.nonce.utf8))))
        let reply = try await backend.post("v1/enrollments", Enrollment(challengeID: challenge.id, sessionID: session,
                                                                          keyID: key, attestation: attest), as: EnrollmentReply.self)
        guard reply.enrolled else { throw URLError(.userAuthenticationRequired) }
        UserDefaults.standard.set(key, forKey: storageKey)
        keyID = key
        return key
    }
}
