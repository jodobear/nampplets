// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "NMPNativeRuntime",
    platforms: [
        .macOS(.v13),
    ],
    products: [
        .library(name: "NMPNativeRuntime", targets: ["NMPNativeRuntime"]),
    ],
    targets: [
        .binaryTarget(
            name: "nmp_native_runtime_ffiFFI",
            path: "NMPNativeRuntime.xcframework"
        ),
        .target(
            name: "NMPNativeRuntime",
            dependencies: ["nmp_native_runtime_ffiFFI"],
            linkerSettings: [
                .linkedFramework("SystemConfiguration", .when(platforms: [.macOS])),
                .linkedFramework("Security", .when(platforms: [.macOS])),
            ]
        ),
        .testTarget(
            name: "NMPNativeRuntimeTests",
            dependencies: ["NMPNativeRuntime"]
        ),
    ]
)
