import XCTest

extension RuntimeWorkbenchUITests {
    @MainActor
    func testActivityUsesAdmittedSourceOnFirstPresentation() throws {
        let app = XCUIApplication()
        app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]
        app.launchEnvironment["NMP_WORKBENCH_UI_TEST_SCENARIO"] =
            "good-morning-permission-launch"
        app.launch()
        app.activate()

        // The published Good Morning manifest declares no `requires` tags, so
        // no runtime code special-cases it into a capability profile anymore:
        // it installs with an empty review and lands directly on the canvas
        // with no permission sheet to dismiss first.
        XCTAssertTrue(
            app.groups["bundled-napplet"].waitForExistence(timeout: 10),
            "an artifact with no required capabilities must launch unconditionally"
        )

        // Every wait in this sequence uses the suite's standard budget. A
        // shorter one is not a meaningful optimisation: it only decides which
        // step trips first when the machine is loaded, and these run against
        // the shared desktop session (see #137, #147).
        // Queried by accessibility identifier, like every other control in
        // this suite. A `menuButtons["Workspace Actions"]` label query does
        // not match: the menu is `.labelStyle(.iconOnly)`, so its rendered
        // element carries neither that title nor the `menuButton` type.
        let workspaceActions = app.descendants(matching: .any)[
            "workspace-actions"
        ]
        XCTAssertTrue(
            workspaceActions.waitForExistence(timeout: 10),
            "The workspace actions menu must appear once the napplet is on the canvas"
        )
        workspaceActions.click()
        let activity = app.menuItems["Activity"]
        XCTAssertTrue(
            activity.waitForExistence(timeout: 10),
            "The workspace actions menu must offer the Activity item"
        )
        activity.click()

        let drawer = app.descendants(matching: .any)
            .matching(
                NSPredicate(
                    format: "label BEGINSWITH %@ OR value BEGINSWITH %@",
                    "Activity for exact build good-morning",
                    "Activity for exact build good-morning"
                )
            )
            .firstMatch
        XCTAssertTrue(
            drawer.waitForExistence(timeout: 10),
            "The first Activity presentation must show its admitted exact build"
        )
        XCTAssertFalse(app.staticTexts["Activity unavailable"].exists)
    }
}
