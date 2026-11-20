import Foundation
import CryptoKit
import Security

/// Device key (Secure Enclave P-256), manifest signing and HPKE share sealing (plan C2, C6).
nonisolated enum POVCrypto {
    static let keyDefaultsKey = "pov.seKey"
    static let shareInfo = Data("pov-share-v1".utf8)

    /// The capture-signing key. Created inside the Secure Enclave on first use; the private key never
    /// leaves it, UserDefaults only holds the enclave-wrapped handle (`dataRepresentation`).
    static func deviceKey() throws -> SecureEnclave.P256.Signing.PrivateKey {
        guard SecureEnclave.isAvailable else { throw POVError.notReady("Secure Enclave unavailable") }
        let defaults = UserDefaults.standard
        if let stored = defaults.data(forKey: keyDefaultsKey),
           let key = try? SecureEnclave.P256.Signing.PrivateKey(dataRepresentation: stored) {
            return key
        }
        let key = try SecureEnclave.P256.Signing.PrivateKey()
        defaults.set(key.dataRepresentation, forKey: keyDefaultsKey)
        return key
    }

    /// 33-byte compressed SEC1 public key, lowercase hex (66 chars, `02`/`03` prefix).
    static func pubkeyHex(_ key: SecureEnclave.P256.Signing.PrivateKey) -> String {
        hex(key.publicKey.compressedRepresentation)
    }

    /// ECDSA P-256/SHA-256 over `data`; 64-byte r‖s, lowercase hex.
    static func sign(_ data: Data, key: SecureEnclave.P256.Signing.PrivateKey) throws -> String {
        hex(try key.signature(for: data).rawRepresentation)
    }

    static func sha256(_ data: Data) -> Data {
        Data(SHA256.hash(data: data))
    }

    static func sha256Hex(_ data: Data) -> String {
        hex(SHA256.hash(data: data))
    }

    /// RFC 9180 base mode, DHKEM(X25519, HKDF-SHA256) / HKDF-SHA256 / ChaCha20-Poly1305, info `pov-share-v1`.
    static func sealShare(_ share: Data, hpkePkHex: String, aad: Data) throws -> (enc: Data, ct: Data) {
        guard let raw = unhex(hpkePkHex), raw.count == 32 else { throw POVError.notReady("bad node HPKE key") }
        let recipient = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: raw)
        var sender = try HPKE.Sender(recipientKey: recipient, ciphersuite: .Curve25519_SHA256_ChachaPoly, info: shareInfo)
        let ct = try sender.seal(share, authenticating: aad)
        return (sender.encapsulatedKey, ct)
    }

    static func hex(_ bytes: some Sequence<UInt8>) -> String {
        let digits = Array("0123456789abcdef".utf8)
        var out = [UInt8]()
        for byte in bytes {
            out.append(digits[Int(byte >> 4)])
            out.append(digits[Int(byte & 0x0F)])
        }
        return String(decoding: out, as: UTF8.self)
    }

    static func unhex(_ string: String) -> Data? {
        let chars = Array(string.utf8)
        guard chars.count % 2 == 0 else { return nil }
        func nibble(_ c: UInt8) -> UInt8? {
            switch c {
            case 48...57: return c - 48
            case 65...70: return c - 55
            case 97...102: return c - 87
            default: return nil
            }
        }
        var out = Data(capacity: chars.count / 2)
        var i = 0
        while i < chars.count {
            guard let hi = nibble(chars[i]), let lo = nibble(chars[i + 1]) else { return nil }
            out.append(hi << 4 | lo)
            i += 2
        }
        return out
    }
}
