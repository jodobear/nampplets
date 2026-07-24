import Foundation
import XCTest

final class RuntimeWorkbenchUITests: XCTestCase {
    private static let liveCatalogOptInMarker =
        "/tmp/nampplets-run-live-catalog-ui-test"
    private static let maximumLiveReviewAttempts = 8

    override func setUpWithError() throws {
        // Put setup code here. This method is called before the invocation of each test method in the class.

        // In UI tests it is usually best to stop immediately when a failure occurs.
        continueAfterFailure = false

        // In UI tests it’s important to set the initial state - such as interface orientation - required for your tests before they run. The setUp method is a good place to do this.
    }

    override func tearDownWithError() throws {
        // Put teardown code here. This method is called after the invocation of each test method in the class.
    }

    @MainActor
    func testWorkbenchReviewsPermissionsThenLaunchesSignedGoodMorning() throws {
        let app = XCUIApplication()
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]
        app.launchEnvironment["NMP_WORKBENCH_UI_TEST_SCENARIO"] =
            "good-morning-permission-launch"
        app.launch()

        let initialPermissionConfirm = app.buttons["permission-confirm"]
        XCTAssertTrue(
            initialPermissionConfirm.waitForExistence(timeout: 10),
            "The exact build must enter native permission review"
        )
        let cancelInitialReview = app.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancelInitialReview.waitForExistence(timeout: 2))
        cancelInitialReview.click()
        let reopenReview = app.buttons["Review Permissions"]
        XCTAssertTrue(
            reopenReview.waitForExistence(timeout: 10),
            "Installation must place a recoverable permission action on the canvas"
        )
        reopenReview.click()

        for domain in ["identity", "inc", "outbox"] {
            let decision = app.descendants(matching: .any)[
                "permission-decision-\(domain)"
            ]
            XCTAssertTrue(decision.waitForExistence(timeout: 10))
            decision.click()
            let allow = app.descendants(matching: .any)[
                "permission-\(domain)-allowExactBuild"
            ]
            XCTAssertTrue(allow.waitForExistence(timeout: 2))
            allow.click()
        }

        let confirm = app.descendants(matching: .any)["permission-confirm"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 2))
        confirm.click()

        XCTAssertTrue(
            app.groups["bundled-napplet"].waitForExistence(timeout: 10)
        )
        XCTAssertTrue(
            app.radioGroups["View mode"].waitForExistence(timeout: 10),
            "Good Morning must pass its essential NAP check after launch"
        )
        XCTAssertFalse(
            app.staticTexts["good-morning can't start here"].exists
        )
        XCTAssertEqual(
            app.staticTexts.matching(
                NSPredicate(
                    format: "value CONTAINS %@ OR label CONTAINS %@",
                    "NAP-OUTBOX",
                    "NAP-OUTBOX"
                )
            ).count,
            0,
            "No full or partial runtime warning may report NAP-OUTBOX absent"
        )
    }

    @MainActor
    func testLiveCatalogOpensVerifiedNetworkNappletReview() throws {
        try XCTSkipUnless(
            Self.liveCatalogTestIsEnabled,
            "The live relay-backed catalog journey is opt-in. Set "
                + "NMP_RUN_LIVE_CATALOG_UI_TEST=1 or create "
                + Self.liveCatalogOptInMarker
        )

        let app = XCUIApplication()
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]
        app.launch()

        let addNapplet = app.descendants(matching: .any)["add-napplet"]
        XCTAssertTrue(addNapplet.waitForExistence(timeout: 10))
        addNapplet.click()

        let liveScope = app.descendants(matching: .any)[
            "catalog-feed-evidence"
        ]
        XCTAssertTrue(
            liveScope.waitForExistence(timeout: 30),
            "The sheet must identify the permanent feed as a bounded live NMP window"
        )
        XCTAssertTrue(
            liveScope.label.contains("Live NMP catalog window")
                || (liveScope.value as? String)?.contains(
                    "Live NMP catalog window"
                ) == true
        )

        // Keep this a real network journey while selecting a known current
        // public candidate whose signed blob is reachable. The search is a
        // local filter over the permanent bounded window, never a new relay
        // query or a fixture substitution.
        let search = app.textFields["Search napplet catalog"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.click()
        search.typeText("Chesslet")
        app.buttons["Search"].click()

        let catalogEntries = app.buttons.matching(identifier: "catalog-entry")
        XCTAssertTrue(
            catalogEntries.firstMatch.waitForExistence(timeout: 60),
            "The production NMP catalog should project a bounded network result"
        )
        // The permanent expandable window may deliver an initial small page
        // before its next replacement adds more public candidates. Give the
        // subscription one event-driven opportunity to expose the next rows.
        _ = catalogEntries
            .element(boundBy: Self.maximumLiveReviewAttempts - 1)
            .waitForExistence(timeout: 30)

        let attempts = min(
            catalogEntries.count,
            Self.maximumLiveReviewAttempts
        )
        XCTAssertGreaterThan(
            attempts,
            0,
            "The permanent feed must expose at least one network napplet"
        )

        var installedExactBuild = false
        for index in 0 ..< attempts {
            let entry = catalogEntries.element(boundBy: index)
            guard entry.waitForExistence(timeout: 2), entry.isHittable else {
                continue
            }
            entry.click()

            let installExactBuild = app.buttons[
                "catalog-install-exact-build"
            ]
            guard installExactBuild.waitForExistence(timeout: 20) else {
                continue
            }
            guard installExactBuild.isEnabled else {
                dismissCatalogReview(in: app)
                continue
            }

            XCTAssertTrue(
                installExactBuild.label.contains("Install Exact Build"),
                "The review must offer only the frozen exact-build action"
            )
            installExactBuild.click()

            if waitForNonexistence(
                of: installExactBuild,
                timeout: 20
            ) {
                installedExactBuild = true
                break
            }

            // A real source can disappear between review and acquisition.
            // Try another already-bounded feed entry without retrying this
            // consumed exact review.
            dismissCatalogReview(in: app)
        }

        XCTAssertTrue(
            installedExactBuild,
            "At least one bounded network candidate should complete exact verified installation"
        )

        let renderedNapplet = app.groups["bundled-napplet"]
        if renderedNapplet.waitForExistence(timeout: 30) {
            XCTAssertTrue(
                app.groups["napplet-canvas"].exists,
                "A safely launchable network build must render inside the native canvas"
            )
            XCTAssertFalse(
                app.descendants(matching: .any)
                    .matching(
                        NSPredicate(
                            format: "label BEGINSWITH %@ OR value BEGINSWITH %@",
                            "Refused:",
                            "Refused:"
                        )
                    )
                    .firstMatch
                    .exists,
                "The selected real build must not be reported as refused after rendering"
            )
            return
        }

        // Do not approve any capability for a network napplet in this test.
        // Reaching Rust's exact permission review proves installation and is
        // the furthest safe point when launch requires new grants.
        let permissionConfirm = app.buttons["permission-confirm"]
        XCTAssertTrue(
            permissionConfirm.waitForExistence(timeout: 10),
            "An installed build that cannot launch grant-free must enter exact permission review"
        )
        XCTAssertTrue(
            permissionConfirm.isHittable,
            "Permission review must be visibly presented after the catalog closes"
        )
        let cancelReview = app.buttons["Cancel"].firstMatch
        XCTAssertTrue(cancelReview.waitForExistence(timeout: 2))
        cancelReview.click()
        XCTAssertTrue(
            app.buttons["Review Permissions"].waitForExistence(
                timeout: 10
            ),
            "The verified installation must remain as a recoverable canvas window"
        )
    }

    private static var liveCatalogTestIsEnabled: Bool {
        ProcessInfo.processInfo.environment[
            "NMP_RUN_LIVE_CATALOG_UI_TEST"
        ] == "1"
            || FileManager.default.fileExists(
                atPath: liveCatalogOptInMarker
            )
    }

    @MainActor
    private func dismissCatalogReview(in app: XCUIApplication) {
        let cancel = app.buttons.matching(identifier: "Cancel").firstMatch
        guard cancel.waitForExistence(timeout: 2), cancel.isHittable else {
            return
        }
        cancel.click()
        _ = waitForNonexistence(
            of: app.buttons[
                "catalog-install-exact-build"
            ],
            timeout: 5
        )
    }

    @MainActor
    private func waitForNonexistence(
        of element: XCUIElement,
        timeout: TimeInterval
    ) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "exists == false"),
            object: element
        )
        return XCTWaiter.wait(
            for: [expectation],
            timeout: timeout
        ) == .completed
    }
}
