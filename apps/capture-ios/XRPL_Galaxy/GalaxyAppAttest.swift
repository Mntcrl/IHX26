import DeviceCheck
import Foundation

enum GalaxyAppAttest {
    static var isSupported: Bool { DCAppAttestService.shared.isSupported }

    static func generateKeyForRegistration() async throws -> String {
        guard isSupported else { throw FlowError.paused("App Attest is not supported by this iPhone installation.") }
        do { return try await DCAppAttestService.shared.generateKey() }
        catch is CancellationError { throw CancellationError() }
        catch { throw FlowError.paused("Apple could not generate an App Attest key. Check connectivity and resume.") }
    }

    /// The Rust core already hashed the protocol preimage. Never hash the digest again.
    static func attestDigest(_ keyID: String, digest: Data) async throws -> Data {
        guard isSupported, digest.count == 32 else { throw GalaxyError.Attestation }
        do { return try await DCAppAttestService.shared.attestKey(keyID, clientDataHash: digest) }
        catch is CancellationError { throw CancellationError() }
        catch let error as DCError where error.code == .invalidKey || error.code == .invalidInput {
            throw FlowError.failed("Apple rejected this registration key or challenge. Start a new registration attempt.")
        }
        catch { throw FlowError.paused("Apple did not complete App Attest. Check connectivity and resume this registration.") }
    }

    static func assertDigest(_ keyID: String, digest: Data) async throws -> Data {
        guard isSupported, digest.count == 32 else { throw GalaxyError.Attestation }
        do { return try await DCAppAttestService.shared.generateAssertion(keyID, clientDataHash: digest) }
        catch is CancellationError { throw CancellationError() }
        catch let error as DCError where error.code == .invalidKey || error.code == .invalidInput {
            throw FlowError.failed("Apple rejected the registered App Attest key or the capture digest.")
        }
        catch { throw FlowError.paused("Apple did not complete the assertion. Check connectivity and resume the same capture.") }
    }

    #if DEBUG
    enum Failure: Error { case invalidRegistration, invalidCaptureCore }

    // The diagnostic writes the preimage bytes for independent fixture verification.
    // Production certification uses only the prehashed digests above.
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

    static func assertionClientData(core: Data) throws -> Data {
        guard core.count == 299 else { throw Failure.invalidCaptureCore }
        return Data("GALAXY/ASSERT/v1\0".utf8) + core
    }

    #endif
}
