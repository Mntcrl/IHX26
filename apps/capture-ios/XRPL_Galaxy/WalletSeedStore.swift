import Foundation
import Security

/// Holds the 16 bytes of XRPL Ed25519 family-seed entropy. Derivation and signing belong to Rust.
/// Keychain item: generic password, this device only, available while unlocked, never synchronized.
/// The wallet is user data (spec §19): this type adds and reads it, and has no deletion or rotation path.
nonisolated enum WalletSeedStore {
    enum Failure: LocalizedError {
        case invalidSeed, keychain(OSStatus)
        var errorDescription: String? {
            switch self {
            case .invalidSeed: "An XRPL Ed25519 seed must contain exactly 16 bytes of entropy."
            case .keychain(let status): "Wallet Keychain access failed (OSStatus \(status))."
            }
        }
    }

    private static var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: "utcq.XRPL-Galaxy.wallet",
         kSecAttrAccount as String: "xrpl-testnet-ed25519",
         kSecAttrSynchronizable as String: false]
    }

    /// Adds a seed once. A duplicate is an error; this must never rotate an existing wallet implicitly.
    static func store(_ entropy: Data) throws {
        guard entropy.count == 16 else { throw Failure.invalidSeed }
        var attrs = query
        attrs[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
        attrs[kSecValueData as String] = entropy
        let status = SecItemAdd(attrs as CFDictionary, nil)
        guard status == errSecSuccess else { throw Failure.keychain(status) }
    }

    static func load() throws -> Data? {
        var attrs = query
        attrs[kSecReturnData as String] = true
        attrs[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(attrs as CFDictionary, &item)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw Failure.keychain(status) }
        guard let entropy = item as? Data, entropy.count == 16 else { throw Failure.invalidSeed }
        return entropy
    }
}
