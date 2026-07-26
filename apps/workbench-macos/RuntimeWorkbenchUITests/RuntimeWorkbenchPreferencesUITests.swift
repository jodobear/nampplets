import XCTest

extension RuntimeWorkbenchUITests {
    @MainActor
    func testPreferencesUsePlainLanguageAndExposeBoundedChoices() {
        let app = XCUIApplication()
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]
        app.launchEnvironment["NMP_WORKBENCH_UI_TEST_SCENARIO"] =
            "preferences"
        app.launch()
        app.activate()

        XCTAssertTrue(
            app.descendants(matching: .any)["add-napplet"]
                .waitForExistence(timeout: 10)
        )
        XCTAssertFalse(
            app.descendants(matching: .any)["workspace-actions"].exists
        )
        app.typeKey(",", modifierFlags: .command)

        XCTAssertTrue(
            app.staticTexts["General"].waitForExistence(timeout: 10)
        )
        let connections = app.descendants(matching: .any)[
            "settings-connections"
        ]
        XCTAssertTrue(connections.waitForExistence(timeout: 10))
        connections.click()
        XCTAssertTrue(app.staticTexts["App relays"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Indexer relays"].exists)

        let storage = app.descendants(matching: .any)["settings-storage"]
        XCTAssertTrue(storage.waitForExistence(timeout: 10))
        storage.click()
        XCTAssertTrue(app.buttons["settings-clear-network-cache"].exists)
        XCTAssertFalse(app.staticTexts["Runtime profile"].exists)
        XCTAssertFalse(app.staticTexts["Data ownership"].exists)
    }
}
