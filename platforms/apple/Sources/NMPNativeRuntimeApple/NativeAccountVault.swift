import CryptoKit
import Foundation
import Security

struct NativeAccountCredential: Equatable, Sendable {
    let publicKey: String
    let secret: String
}

struct NativeAccountVaultSnapshot: Equatable, Sendable {
    let credentials: [NativeAccountCredential]
    let activePublicKey: String?
}

enum NativeAccountVaultError: Error, Equatable {
    case capacity(limit: Int)
    case corrupt
    case invalidNamespace
    case invalidPublicKey
    case invalidSecret
    case keychain(status: OSStatus)
    case unknownAccount
}

protocol NativeAccountVault: Sendable {
    func load(maximumAccounts: Int) throws -> NativeAccountVaultSnapshot
    func upsert(
        publicKey: String,
        secret: String,
        maximumAccounts: Int
    ) throws
    func setActive(
        publicKey: String?,
        maximumAccounts: Int
    ) throws
    func remove(
        publicKey: String,
        maximumAccounts: Int
    ) throws
}

/// Secure, profile-scoped credential persistence for the native host.
///
/// NMP remains the account and signer authority. This vault only performs the
/// app-owned secure-storage step required to re-register the same local signer
/// after process restart. Neither secrets nor Keychain item data are included
/// in errors, logs, activity, or runtime snapshots.
final class MacOSKeychainAccountVault:
    NativeAccountVault,
    @unchecked Sendable
{
    private static let activeAccountMarker = "__active_public_key__"
    private static let maximumNamespaceBytes = 4_096
    private static let maximumSecretBytes = 1_024

    private let service: String

    init(namespace: String) throws {
        guard
            !namespace.isEmpty,
            namespace.utf8.count <= Self.maximumNamespaceBytes,
            !namespace.unicodeScalars.contains(where: {
                CharacterSet.controlCharacters.contains($0)
            })
        else {
            throw NativeAccountVaultError.invalidNamespace
        }
        service = Self.serviceName(namespace: namespace)
    }

    static func serviceName(namespace: String) -> String {
        let digest = SHA256.hash(data: Data(namespace.utf8))
            .map { String(format: "%02x", $0) }
            .joined()
        return "io.f7z.nmp-native-runtime.accounts.\(digest)"
    }

    func load(
        maximumAccounts: Int
    ) throws -> NativeAccountVaultSnapshot {
        guard maximumAccounts > 0 else {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        let items = try readItems()
        var credentials: [NativeAccountCredential] = []
        var seen = Set<String>()
        var activePublicKey: String?

        for item in items {
            guard
                let account = item[kSecAttrAccount as String] as? String,
                let data = item[kSecValueData as String] as? Data,
                let value = String(data: data, encoding: .utf8)
            else {
                throw NativeAccountVaultError.corrupt
            }
            if account == Self.activeAccountMarker {
                guard activePublicKey == nil else {
                    throw NativeAccountVaultError.corrupt
                }
                activePublicKey = try Self.normalizedPublicKey(value)
                continue
            }

            let publicKey = try Self.normalizedPublicKey(account)
            try Self.validateSecret(value)
            guard seen.insert(publicKey).inserted else {
                throw NativeAccountVaultError.corrupt
            }
            credentials.append(
                NativeAccountCredential(
                    publicKey: publicKey,
                    secret: value
                )
            )
            guard credentials.count <= maximumAccounts else {
                throw NativeAccountVaultError.capacity(limit: maximumAccounts)
            }
        }

        credentials.sort { $0.publicKey < $1.publicKey }
        if
            let activePublicKey,
            !seen.contains(activePublicKey)
        {
            // A stale marker cannot activate an identity without a credential.
            // Treat it as logged out while preserving the remaining accounts.
            return NativeAccountVaultSnapshot(
                credentials: credentials,
                activePublicKey: nil
            )
        }
        return NativeAccountVaultSnapshot(
            credentials: credentials,
            activePublicKey: activePublicKey
        )
    }

    func upsert(
        publicKey: String,
        secret: String,
        maximumAccounts: Int
    ) throws {
        let publicKey = try Self.normalizedPublicKey(publicKey)
        try Self.validateSecret(secret)
        let snapshot = try load(maximumAccounts: maximumAccounts)
        if
            !snapshot.credentials.contains(where: {
                $0.publicKey == publicKey
            }),
            snapshot.credentials.count >= maximumAccounts
        {
            throw NativeAccountVaultError.capacity(limit: maximumAccounts)
        }
        try store(
            account: publicKey,
            data: Data(secret.utf8)
        )
    }

    func setActive(
        publicKey: String?,
        maximumAccounts: Int
    ) throws {
        guard let requestedPublicKey = publicKey else {
            try delete(account: Self.activeAccountMarker)
            return
        }
        let publicKey = try Self.normalizedPublicKey(
            requestedPublicKey
        )
        let snapshot = try load(maximumAccounts: maximumAccounts)
        guard snapshot.credentials.contains(where: {
            $0.publicKey == publicKey
        }) else {
            throw NativeAccountVaultError.unknownAccount
        }
        try store(
            account: Self.activeAccountMarker,
            data: Data(publicKey.utf8)
        )
    }

    func remove(
        publicKey: String,
        maximumAccounts: Int
    ) throws {
        let publicKey = try Self.normalizedPublicKey(publicKey)
        let activeBeforeRemoval = try load(
            maximumAccounts: maximumAccounts
        ).activePublicKey
        try delete(account: publicKey)
        if activeBeforeRemoval == publicKey {
            try delete(account: Self.activeAccountMarker)
        }
    }

    private func readItems() throws -> [[String: Any]] {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecMatchLimit as String: kSecMatchLimitAll,
            kSecReturnAttributes as String: true,
            kSecReturnData as String: true,
        ]
        var raw: CFTypeRef?
        let status = SecItemCopyMatching(
            query as CFDictionary,
            &raw
        )
        if status == errSecItemNotFound {
            return []
        }
        guard status == errSecSuccess else {
            throw NativeAccountVaultError.keychain(status: status)
        }
        if let items = raw as? [[String: Any]] {
            return items
        }
        if let item = raw as? [String: Any] {
            return [item]
        }
        throw NativeAccountVaultError.corrupt
    }

    private func store(account: String, data: Data) throws {
        let selector: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let updateStatus = SecItemUpdate(
            selector as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess {
            return
        }
        guard updateStatus == errSecItemNotFound else {
            throw NativeAccountVaultError.keychain(
                status: updateStatus
            )
        }
        var item = selector
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] =
            kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(item as CFDictionary, nil)
        guard addStatus == errSecSuccess else {
            throw NativeAccountVaultError.keychain(status: addStatus)
        }
    }

    private func delete(account: String) throws {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw NativeAccountVaultError.keychain(status: status)
        }
    }

    static func normalizedPublicKey(
        _ value: String
    ) throws -> String {
        let normalized = value.lowercased()
        guard
            normalized.utf8.count == 64,
            normalized.unicodeScalars.allSatisfy({
                (48 ... 57).contains($0.value)
                    || (97 ... 102).contains($0.value)
            })
        else {
            throw NativeAccountVaultError.invalidPublicKey
        }
        return normalized
    }

    static func validateSecret(_ value: String) throws {
        guard
            !value.isEmpty,
            value.utf8.count <= Self.maximumSecretBytes,
            !value.unicodeScalars.contains(where: {
                CharacterSet.controlCharacters.contains($0)
            })
        else {
            throw NativeAccountVaultError.invalidSecret
        }
    }
}
