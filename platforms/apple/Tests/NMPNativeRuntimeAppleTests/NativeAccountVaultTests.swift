@testable import NMPNativeRuntimeApple
import Testing

@Test func keychainNamespaceIsStableBoundedAndDoesNotEmbedThePath() {
    let namespace = "/private/runtime/profile-a"
    let first = MacOSKeychainAccountVault.serviceName(
        namespace: namespace
    )
    let repeated = MacOSKeychainAccountVault.serviceName(
        namespace: namespace
    )
    let other = MacOSKeychainAccountVault.serviceName(
        namespace: "/private/runtime/profile-b"
    )

    #expect(first == repeated)
    #expect(first != other)
    #expect(!first.contains(namespace))
    #expect(first.utf8.count == 99)
}

@Test func vaultInputValidationIsBoundedAndSecretSafe() throws {
    let uppercase = String(repeating: "AB", count: 32)
    let normalized = try MacOSKeychainAccountVault.normalizedPublicKey(
        uppercase
    )
    #expect(normalized == uppercase.lowercased())

    #expect(throws: NativeAccountVaultError.invalidPublicKey) {
        try MacOSKeychainAccountVault.normalizedPublicKey("npub1nothex")
    }
    #expect(throws: NativeAccountVaultError.invalidPublicKey) {
        try MacOSKeychainAccountVault.normalizedPublicKey(
            String(repeating: "g", count: 64)
        )
    }
    #expect(throws: NativeAccountVaultError.invalidSecret) {
        try MacOSKeychainAccountVault.validateSecret("")
    }
    #expect(throws: NativeAccountVaultError.invalidSecret) {
        try MacOSKeychainAccountVault.validateSecret("secret\nvalue")
    }
    #expect(throws: NativeAccountVaultError.invalidSecret) {
        try MacOSKeychainAccountVault.validateSecret(
            String(repeating: "s", count: 1_025)
        )
    }

    try MacOSKeychainAccountVault.validateSecret(
        String(repeating: "a", count: 64)
    )
}
