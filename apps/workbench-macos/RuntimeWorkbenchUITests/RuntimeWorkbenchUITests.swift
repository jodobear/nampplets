import XCTest

final class RuntimeWorkbenchUITests: XCTestCase {

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
        app.launchEnvironment["NMP_WORKBENCH_UI_TEST_SCENARIO"] =
            "good-morning-permission-launch"
        app.launch()

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
        XCTAssertFalse(app.staticTexts["NAP-OUTBOX"].exists)
    }
}
