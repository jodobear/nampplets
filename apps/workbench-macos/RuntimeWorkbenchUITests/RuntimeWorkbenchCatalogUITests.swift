import Foundation
import XCTest

extension RuntimeWorkbenchUITests {
    @MainActor
    func testLiveCatalogInstallsAndMountsVerifiedNetworkNapplet() throws {
        try XCTSkipUnless(
            Self.liveCatalogTestIsEnabled,
            "The live relay-backed catalog journey is opt-in. Set "
                + "NMP_RUN_LIVE_CATALOG_UI_TEST=1 or create "
                + Self.liveCatalogOptInMarker
        )

        let app = XCUIApplication()
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]
        isolateStorage(of: app)
        app.launch()
        app.activate()

        let addNapplet = app.descendants(matching: .any)["add-napplet"]
        XCTAssertTrue(addNapplet.waitForExistence(timeout: 10))
        addNapplet.click()

        // The bounded live-window scope remains available one step away.
        let feedEvidence = app.descendants(matching: .any)[
            "Where these came from"
        ].firstMatch
        XCTAssertTrue(
            feedEvidence.waitForExistence(timeout: 30),
            "The feed must offer its source evidence"
        )
        feedEvidence.click()
        XCTAssertTrue(
            app.staticTexts.containing(
                NSPredicate(format: "value CONTAINS %@", "live NMP window")
            ).firstMatch.waitForExistence(timeout: 10),
            "Opening the evidence must name the bounded live window verbatim"
        )

        // Search is a local filter over the permanent bounded window.
        let search = app.textFields["Filter these napplets"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.click()
        search.typeText("STL Preview")
        search.typeText("\r")

        let catalogEntries = app.buttons.matching(identifier: "catalog-entry")
        XCTAssertTrue(
            catalogEntries.firstMatch.waitForExistence(timeout: 60),
            "The production NMP catalog should project a bounded network result"
        )
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
                installExactBuild.label.contains("Add Napplet"),
                "The review must offer a single install action"
            )
            installExactBuild.click()

            if waitForNonexistence(
                of: installExactBuild,
                timeout: 20
            ) {
                installedExactBuild = true
                break
            }

            // A live source can disappear between review and acquisition.
            dismissCatalogReview(in: app)
        }

        XCTAssertTrue(
            installedExactBuild,
            "At least one candidate should complete exact verified installation"
        )

        let permissionConfirm = app.buttons["permission-confirm"]
        XCTAssertTrue(
            permissionConfirm.waitForExistence(timeout: 10),
            "The installed STL Preview build must enter exact permission review"
        )
        XCTAssertTrue(
            permissionConfirm.isHittable,
            "Permission review must be visibly presented after the catalog closes"
        )
        XCTAssertTrue(permissionConfirm.isEnabled)
        permissionConfirm.click()
        XCTAssertTrue(
            waitForNonexistence(of: permissionConfirm, timeout: 20),
            "Rust's recommended exact permission batch must apply before launch"
        )

        XCTAssertTrue(
            app.staticTexts["Waiting for an STL to preview..."]
                .waitForExistence(timeout: 30),
            "The public napplet's DOM must mount and pass NAP-INC readiness"
        )
        XCTAssertTrue(
            app.descendants(matching: .any)["STL Preview title bar"]
                .waitForExistence(timeout: 10),
            "The mounted public napplet must remain in a native window"
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
            "The installed public build must not be reported as refused"
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
}
