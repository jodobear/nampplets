import XCTest

extension RuntimeWorkbenchUITests {
    /// Every presence wait in this flow uses the suite's standard 10s
    /// allowance. Each step waits on a window-server transition that
    /// XCTest's automatic app-idle wait cannot cover.
    @MainActor
    func registerAndActivateDeterministicAccount(
        in app: XCUIApplication
    ) {
        let accountSwitcher = app.descendants(matching: .any)[
            "account-switcher"
        ]
        XCTAssertTrue(
            accountSwitcher.waitForExistence(timeout: 10),
            "The account switcher must be the first toolbar control"
        )
        accountSwitcher.click()

        let addSigner = app.menuItems["Add Account…"]
        XCTAssertTrue(
            addSigner.waitForExistence(timeout: 10),
            "The account menu must offer a single add-account intent"
        )
        addSigner.click()

        let identityField = app.textFields["account-identity"]
        XCTAssertTrue(identityField.waitForExistence(timeout: 10))
        identityField.click()
        identityField.typeText(Self.uiTestSigningSecret)

        let continueButton = app.buttons["account-add-continue"]
        XCTAssertTrue(
            continueButton.waitForExistence(timeout: 10),
            "The account sheet must offer one Continue control"
        )
        XCTAssertTrue(continueButton.isEnabled)
        continueButton.click()
        XCTAssertTrue(
            waitForNonexistence(of: continueButton, timeout: 10),
            "Continue must add, select, and dismiss without an Activate step"
        )

        let selectedAccount = app.descendants(matching: .any)[
            "account-switcher"
        ]
        XCTAssertTrue(selectedAccount.waitForExistence(timeout: 10))
        XCTAssertEqual(selectedAccount.value as? String, "Signing Account")
        XCTAssertFalse(
            app.staticTexts[Self.uiTestSigningPublicKey].exists,
            "The toolbar must not render the account public key as identity"
        )
    }
}
