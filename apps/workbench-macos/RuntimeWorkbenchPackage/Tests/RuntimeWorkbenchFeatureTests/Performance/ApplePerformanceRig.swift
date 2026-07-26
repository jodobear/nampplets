import Foundation
import NMPNativeRuntimeApple
@testable import RuntimeWorkbenchFeature

enum ApplePerformanceRunState: String, Equatable, Sendable {
    case cold
    case warm
}

struct ApplePerformanceProtocol: Equatable, Sendable {
    let warmupCount: Int
    let sampleCount: Int
    let perSampleDeadlineNanoseconds: UInt64
    let runDeadlineNanoseconds: UInt64
    let outlierPolicy = "tukey_upper_3_iqr_v1"

    static let ordinary = Self(
        warmupCount: 2,
        sampleCount: 4,
        perSampleDeadlineNanoseconds: 5_000_000_000,
        runDeadlineNanoseconds: 60_000_000_000
    )
}

enum ApplePerformanceOutcome: Equatable, Sendable {
    case success
    case refused(domain: String, code: String)
    case failed(code: String)
    case deadlineExceeded
}

struct ApplePerformanceSample: Equatable, Sendable {
    let sequence: Int
    let durationNanoseconds: UInt64
    let cpuTimeNanoseconds: UInt64?
    let peakRSSBytes: UInt64?
    let outcome: ApplePerformanceOutcome
}

struct ApplePerformanceRun: Equatable, Sendable {
    let benchmarkID: String
    let state: ApplePerformanceRunState
    let resetScopes: [String]
    let fixtureID: String
    let fixtureSHA256: String
    let fixtureCardinality: UInt64
    let performanceProtocol: ApplePerformanceProtocol
    let environment: ApplePerformanceEnvironment
    let samples: [ApplePerformanceSample]

    var refusalCount: Int {
        samples.reduce(into: 0) { count, sample in
            if case .refused = sample.outcome {
                count += 1
            }
        }
    }

    var successfulDurations: [UInt64] {
        samples.compactMap { sample in
            sample.outcome == .success
                ? sample.durationNanoseconds
                : nil
        }
    }
}

@MainActor
struct ApplePerformanceRig {
    static let benchmarkID =
        "apple.native-profile.permission-review.v1"

    let fixture: GoodMorningFixture
    let performanceProtocol: ApplePerformanceProtocol

    init(
        fixture: GoodMorningFixture,
        performanceProtocol: ApplePerformanceProtocol = .ordinary
    ) {
        self.fixture = fixture
        self.performanceProtocol = performanceProtocol
    }

    func run(state: ApplePerformanceRunState) throws -> ApplePerformanceRun {
        let started = DispatchTime.now().uptimeNanoseconds
        let samples: [ApplePerformanceSample]
        switch state {
        case .cold:
            for _ in 0..<performanceProtocol.warmupCount {
                _ = try coldAttempt(sequence: nil)
                try enforceRunDeadline(started)
            }
            samples = try (0..<performanceProtocol.sampleCount).map {
                let sample = try coldAttempt(sequence: $0)
                try enforceRunDeadline(started)
                return sample
            }
        case .warm:
            samples = try warmSamples(runStarted: started)
        }
        return ApplePerformanceRun(
            benchmarkID: Self.benchmarkID,
            state: state,
            resetScopes: state == .cold
                ? ["runtime_store", "nmp_store", "artifact_cache"]
                : ["permission_review"],
            fixtureID: "good-morning-exact-build-v1",
            fixtureSHA256: GoodMorningFixture.aggregateHash,
            fixtureCardinality: 1,
            performanceProtocol: performanceProtocol,
            environment: .capture(),
            samples: samples
        )
    }

    private func warmSamples(
        runStarted: UInt64
    ) throws -> [ApplePerformanceSample] {
        let owner = try openOwner()
        defer { owner.close() }
        let installed = try fixture.install(profile: owner)
        for _ in 0..<performanceProtocol.warmupCount {
            _ = measure(
                sequence: nil,
                profile: owner.native,
                coordinate: installed.permissionCoordinate
            )
            try enforceRunDeadline(runStarted)
        }
        return try (0..<performanceProtocol.sampleCount).map { sequence in
            let sample = measure(
                sequence: sequence,
                profile: owner.native,
                coordinate: installed.permissionCoordinate
            )
            try enforceRunDeadline(runStarted)
            return sample
        }
    }

    private func coldAttempt(
        sequence: Int?
    ) throws -> ApplePerformanceSample {
        let owner = try openOwner()
        defer { owner.close() }
        let installed = try fixture.install(profile: owner)
        return measure(
            sequence: sequence,
            profile: owner.native,
            coordinate: installed.permissionCoordinate
        )
    }

    private func openOwner() throws -> WorkbenchRuntimeProfile {
        try WorkbenchRuntimeProfile.open(
            storageRoot: temporaryRuntimeRoot()
        )
    }

    private func measure(
        sequence: Int?,
        profile: NativeRuntimeProfile,
        coordinate: NativeRuntimePermissionCoordinate
    ) -> ApplePerformanceSample {
        let before = AppleProcessMeasurement.capture()
        let started = DispatchTime.now().uptimeNanoseconds
        let result = profile.permissionReview(for: coordinate)
        let finished = DispatchTime.now().uptimeNanoseconds
        let after = AppleProcessMeasurement.capture()
        let duration = finished >= started ? finished - started : 0
        let outcome: ApplePerformanceOutcome
        if duration >= performanceProtocol.perSampleDeadlineNanoseconds {
            outcome = .deadlineExceeded
        } else if let refusal = result.refusal {
            outcome = .refused(
                domain: "runtime.permission_review",
                code: refusal.code
            )
        } else if result.review == nil {
            outcome = .failed(code: "missing_typed_review")
        } else {
            outcome = .success
        }
        return ApplePerformanceSample(
            sequence: sequence ?? 0,
            durationNanoseconds: duration,
            cpuTimeNanoseconds: difference(
                after.cpuTimeNanoseconds,
                before.cpuTimeNanoseconds
            ),
            peakRSSBytes: after.peakRSSBytes,
            outcome: outcome
        )
    }

    private func difference(
        _ after: UInt64?,
        _ before: UInt64?
    ) -> UInt64? {
        guard let after, let before, after >= before else {
            return nil
        }
        return after - before
    }

    private func enforceRunDeadline(_ started: UInt64) throws {
        let now = DispatchTime.now().uptimeNanoseconds
        guard
            now >= started,
            now - started < performanceProtocol.runDeadlineNanoseconds
        else {
            throw ApplePerformanceRigError.runDeadlineExceeded
        }
    }
}

enum ApplePerformanceRigError: Error {
    case runDeadlineExceeded
}
