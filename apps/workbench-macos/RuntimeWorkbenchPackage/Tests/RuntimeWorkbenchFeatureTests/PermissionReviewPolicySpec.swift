import Nimble
import Quick
@testable import RuntimeWorkbenchFeature

final class PermissionReviewPolicySpec: QuickSpec {
    override class func spec() {
        describe("Native permission review") {
            context("given Rust projects an exact-build capability review") {
                it("renders Rust's grant and recommendation without ranking options") {
                    let snapshot = permissionSnapshot()
                    let model = PermissionReviewSheetModel(
                        manager: RecordingPermissionManager(snapshot: snapshot)
                    )
                    let identity = model.review.capabilities[0]

                    expect(model.isGranted(identity))
                        .to(equal(identity.isGranted))
                    expect(model.selection(for: identity))
                        .to(equal(identity.requestedDecision))

                    expect(identity.option(for: .allowSession)?.isValid)
                        .to(beTrue())
                    expect(identity.recommendedDecision)
                        .to(equal(.allowExactBuild))

                    model.setGranted(true, for: identity)

                    expect(model.selection(for: identity))
                        .to(equal(identity.recommendedDecision))
                    expect(model.selection(for: identity))
                        .toNot(equal(.allowSession))
                    expect(model.isGranted(identity)).to(beTrue())
                }

                it("renders a managed grant without inventing a native choice") {
                    let snapshot = mixedManagedPermissionSnapshot()
                    let model = PermissionReviewSheetModel(
                        manager: RecordingPermissionManager(snapshot: snapshot)
                    )
                    let managed = model.review.capabilities[0]

                    expect(managed.isGranted).to(beTrue())
                    expect(model.isGranted(managed))
                        .to(equal(managed.isGranted))
                    expect(model.selection(for: managed)).to(beNil())
                    expect(model.hasAffirmativeOption(managed)).to(beFalse())
                    expect(model.managedCapabilities.map(\.domain))
                        .to(equal(["identity"]))
                }
            }
        }
    }
}
