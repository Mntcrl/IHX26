import CryptoKit
import DeviceCheck
import Foundation
import Security

@MainActor
enum GalaxyAppAttest {
    enum Failure: LocalizedError {
        case unsupported, invalidRegistration, invalidCaptureCore, random(OSStatus)

        var errorDescription: String? {
            switch self {
            case .unsupported: "App Attest is not supported by this installation."
            case .invalidRegistration: "Invalid App Attest registration input."
            case .invalidCaptureCore: "Invalid App Attest capture core length."
            case .random(let status): "Could not generate registration nonce (OSStatus \(status))."
            }
        }
    }

    static var isSupported: Bool { DCAppAttestService.shared.isSupported }

    static func generateKey() async throws -> String {
        guard isSupported else { throw Failure.unsupported }
        return try await DCAppAttestService.shared.generateKey()
    }

    static func registrationClientData(network: UInt32, ledger: UInt32, ledgerHash: Data,
                                       nonce: Data, keyID: String, account: Data) throws -> Data {
        guard ledgerHash.count == 32, nonce.count == 32, account.count == 20,
              let key = Data(base64Encoded: keyID), key.count == 32 else {
            throw Failure.invalidRegistration
        }
        var data = Data("GALAXY/DEVICE-REG/v1\0".utf8)
        for var value in [network.bigEndian, ledger.bigEndian] {
            withUnsafeBytes(of: &value) { data.append(contentsOf: $0) }
        }
        data.append(ledgerHash)
        data.append(nonce)
        data.append(key)
        data.append(account)
        return data
    }

    static func newNonce() throws -> Data {
        var nonce = Data(count: 32)
        let status = nonce.withUnsafeMutableBytes { (bytes: UnsafeMutableRawBufferPointer) in
            SecRandomCopyBytes(kSecRandomDefault, bytes.count, bytes.baseAddress!)
        }
        guard status == errSecSuccess else { throw Failure.random(status) }
        return nonce
    }

    static func attest(_ keyID: String, clientData: Data) async throws -> Data {
        guard isSupported else { throw Failure.unsupported }
        return try await DCAppAttestService.shared.attestKey(keyID, clientDataHash: digest(clientData))
    }

    static func assertionClientData(core: Data) throws -> Data {
        guard core.count == 299 else { throw Failure.invalidCaptureCore }
        return Data("GALAXY/ASSERT/v1\0".utf8) + core
    }

    static func assert(_ keyID: String, clientData: Data) async throws -> Data {
        guard isSupported else { throw Failure.unsupported }
        return try await DCAppAttestService.shared.generateAssertion(keyID, clientDataHash: digest(clientData))
    }

    private static func digest(_ data: Data) -> Data { Data(SHA256.hash(data: data)) }
}
