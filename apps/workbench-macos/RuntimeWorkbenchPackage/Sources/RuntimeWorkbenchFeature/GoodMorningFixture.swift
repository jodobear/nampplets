import Foundation
import NMPNativeRuntimeApple

enum GoodMorningFixtureError: Error, LocalizedError {
    case missingResource(String)

    var errorDescription: String? {
        switch self {
        case let .missingResource(name):
            "The bundled signed fixture is missing \(name)."
        }
    }
}

struct GoodMorningFixture: Sendable {
    static let author =
        "266815e0c9210dfa324c6cba3573b14bee49da4209a9456f9484e5106cd408a5"
    static let dTag = "good-morning"
    static let indexDigest =
        "ffd35eea5c84d03cdda74c23e1bbb2c40500f503833503aa688036faa52f3808"

    let eventJSON: Data
    let indexHTML: Data

    static func load() throws -> Self {
        let eventURL = try resourceURL(name: "event", extension: "json")
        let indexURL = try resourceURL(name: "index", extension: "html")
        return Self(
            eventJSON: try Data(contentsOf: eventURL),
            indexHTML: try Data(contentsOf: indexURL)
        )
    }

    func open(storageRoot: URL) throws -> NappletArtifact {
        try NappletArtifact.openSignedNamed(
            title: "Good Morning Protocol",
            eventJSON: eventJSON,
            author: Self.author,
            dTag: Self.dTag,
            blobsBySHA256: [Self.indexDigest: indexHTML],
            grantDomains: ["storage"],
            storageRoot: storageRoot
        )
    }

    private static func resourceURL(
        name: String,
        extension pathExtension: String
    ) throws -> URL {
        if let nested = Bundle.module.url(
            forResource: name,
            withExtension: pathExtension,
            subdirectory: "GoodMorning"
        ) {
            return nested
        }
        if let flattened = Bundle.module.url(
            forResource: name,
            withExtension: pathExtension
        ) {
            return flattened
        }
        throw GoodMorningFixtureError.missingResource("\(name).\(pathExtension)")
    }
}
