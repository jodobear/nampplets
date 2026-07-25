// swift-tools-version: 6.1

import PackageDescription

let package = Package(
    name: "NMPNativeRuntimeApple",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(
            name: "NMPNativeRuntimeApple",
            targets: ["NMPNativeRuntimeApple"]
        )
    ],
    dependencies: [
        .package(path: "../../Packages/NMPNativeRuntime")
    ],
    targets: [
        .target(
            name: "NMPNativeRuntimeApple",
            dependencies: [
                .product(
                    name: "NMPNativeRuntime",
                    package: "NMPNativeRuntime"
                )
            ],
            exclude: ["Resources/README.md"],
            resources: [.copy("Resources/TrustedShell")],
            linkerSettings: [
                .linkedFramework("Security", .when(platforms: [.macOS, .iOS]))
            ]
        ),
        .testTarget(
            name: "NMPNativeRuntimeAppleTests",
            dependencies: ["NMPNativeRuntimeApple"],
            resources: [.copy("Fixtures")]
        )
    ]
)
