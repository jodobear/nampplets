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
        .package(path: "../../../platforms/apple"),
        .package(
            url: "https://github.com/Quick/Quick.git",
            from: "7.6.2"
        ),
        .package(
            url: "https://github.com/Quick/Nimble.git",
            from: "13.8.0"
        ),
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
            dependencies: [
                "RuntimeWorkbenchFeature",
                .product(name: "Quick", package: "Quick"),
                .product(name: "Nimble", package: "Nimble"),
            ],
            resources: [.process("Resources")]
        )
    ]
)
