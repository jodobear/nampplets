// swift-tools-version: 6.1

import PackageDescription

let package = Package(
    name: "RuntimeWorkbenchFeature",
    platforms: [.macOS(.v14), .iOS(.v17)],
    products: [
        .library(
            name: "RuntimeWorkbenchFeature",
            targets: ["RuntimeWorkbenchFeature"]
        )
    ],
    dependencies: [
        .package(path: "../../../platforms/apple")
    ],
    targets: [
        .target(
            name: "RuntimeWorkbenchFeature",
            dependencies: [
                .product(
                    name: "NMPNativeRuntimeApple",
                    package: "apple"
                )
            ],
            resources: [.process("Resources")]
        ),
        .testTarget(
            name: "RuntimeWorkbenchFeatureTests",
            dependencies: ["RuntimeWorkbenchFeature"]
        )
    ]
)
