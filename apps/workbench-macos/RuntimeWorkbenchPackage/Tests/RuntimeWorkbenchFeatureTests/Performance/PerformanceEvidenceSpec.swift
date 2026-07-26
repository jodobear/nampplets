import Nimble
import Quick
@testable import RuntimeWorkbenchFeature

final class PerformanceEvidenceSpec: QuickSpec {
    override class func spec() {
        describe("Apple NativeRuntimeProfile performance evidence") {
            context("given the committed Good Morning exact build") {
                it("keeps cold and warm permission-review evidence separate") {
                    let fixture = try GoodMorningFixture.load()
                    let rig = ApplePerformanceRig(fixture: fixture)

                    let cold = try rig.run(state: .cold)
                    let warm = try rig.run(state: .warm)

                    expect(cold.benchmarkID)
                        .to(equal(ApplePerformanceRig.benchmarkID))
                    expect(warm.benchmarkID)
                        .to(equal(ApplePerformanceRig.benchmarkID))
                    expect(cold.state).to(equal(.cold))
                    expect(warm.state).to(equal(.warm))
                    expect(cold.state).toNot(equal(warm.state))
                    expect(cold.samples.count)
                        .to(equal(cold.performanceProtocol.sampleCount))
                    expect(warm.samples.count)
                        .to(equal(warm.performanceProtocol.sampleCount))
                    expect(cold.refusalCount).to(equal(0))
                    expect(warm.refusalCount).to(equal(0))
                    expect(cold.successfulDurations.count)
                        .to(equal(cold.samples.count))
                    expect(warm.successfulDurations.count)
                        .to(equal(warm.samples.count))
                    expect(cold.fixtureSHA256)
                        .to(equal(GoodMorningFixture.aggregateHash))
                    expect(warm.fixtureSHA256)
                        .to(equal(GoodMorningFixture.aggregateHash))
                }
            }
        }
    }
}
