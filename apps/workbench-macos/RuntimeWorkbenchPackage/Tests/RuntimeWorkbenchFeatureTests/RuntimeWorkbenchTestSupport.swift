import Foundation

func temporaryRuntimeRoot() -> URL {
    FileManager.default.temporaryDirectory
        .appendingPathComponent(
            "nmp-native-runtime-workbench-\(UUID().uuidString)",
            isDirectory: true
        )
}
