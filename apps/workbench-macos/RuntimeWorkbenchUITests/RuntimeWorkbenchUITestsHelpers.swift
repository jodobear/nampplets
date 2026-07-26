import Foundation
import XCTest

#if canImport(AppKit)
    import AppKit
#endif

extension RuntimeWorkbenchUITests {
    /// The identifier the Workbench app builds under — and, because it is
    /// hard-coded rather than derived from the checkout, the *same*
    /// identifier in every worktree on the machine
    /// (`apps/workbench-macos/Config/Shared.xcconfig`,
    /// `PRODUCT_BUNDLE_IDENTIFIER`). See issue #147.
    static let workbenchBundleIdentifier = "io.f7z.nmp.runtime-workbench"

    /// Logs whether the app under test was still alive at the moment a
    /// failure was recorded.
    ///
    /// `Failed to get matching snapshots: Lost connection to the
    /// application (pid …)` has two candidate explanations that produce an
    /// identical message and, in both cases, no crash report — so nobody
    /// has been able to tell them apart from the logs alone:
    ///
    /// * **Termination.** Every worktree builds the same
    ///   `PRODUCT_BUNDLE_IDENTIFIER`, and `XCUIApplication.launch()`
    ///   terminates any already-running instance of that bundle id, so
    ///   concurrent UI runs from different worktrees kill each other's app
    ///   (issue #147). Clean termination, no crash report, death at an
    ///   arbitrary point in the victim's timeline.
    /// * **Connection loss.** The AX / `testmanagerd` channel drops under
    ///   load while the app process itself keeps running.
    ///
    /// The discriminator is simply whether the process is still there:
    /// **app gone ⇒ termination; app alive ⇒ connection loss.**
    ///
    /// Two things make the answer trustworthy. First, this runs from
    /// `record(_:)`, at failure time, not from `tearDown` — see the
    /// override for why that matters. Second, it checks the *specific* pid
    /// named in the failure message wherever one is present, not just "is
    /// something with this bundle id running": under the termination
    /// hypothesis the killer's own app is running under that same bundle
    /// id, so a bundle-id-only lookup would report "alive" for the very
    /// case it is supposed to detect. The bundle-id census is logged
    /// alongside as corroboration — a surviving peer instance with a
    /// different pid is the termination signature.
    ///
    /// Costs nothing on a green run: `record(_:)` only fires on failure.
    func logAppLivenessDiagnostic(for issue: XCTIssue) {
        let description = issue.compactDescription
        var fields = [
            String(
                format: "elapsed=%.2fs",
                Date().timeIntervalSince(testStartedAt)
            ),
            "bundleID=\(Self.workbenchBundleIdentifier)",
        ]

        if let pid = Self.applicationPID(inFailureDescription: description) {
            let alive = Self.processExists(pid)
            fields.append("reportedPID=\(pid)")
            fields.append("reportedPIDAlive=\(alive)")
            fields.append(
                "verdict="
                    + (alive
                        ? "app-alive-so-connection-loss"
                        : "app-gone-so-termination")
            )
        } else {
            fields.append("reportedPID=none")
            fields.append("verdict=no-pid-in-failure-message")
        }

        fields.append(
            "runningInstancePIDs=\(Self.runningWorkbenchProcessIdentifiers())"
        )

        NSLog(
            "app-liveness-at-failure: %@ | issue=%@",
            fields.joined(separator: " "),
            description
        )
    }

    /// The pid XCTest names in `Lost connection to the application (pid N)`,
    /// when the failure carries one.
    static func applicationPID(
        inFailureDescription description: String
    ) -> pid_t? {
        guard
            let range = description.range(
                of: #"pid\s+\d+"#,
                options: [.regularExpression, .caseInsensitive]
            )
        else {
            return nil
        }
        return pid_t(description[range].drop { !$0.isNumber })
    }

    /// Whether `pid` still names a live process. `EPERM` counts as alive:
    /// the process exists, this one simply may not signal it.
    static func processExists(_ pid: pid_t) -> Bool {
        kill(pid, 0) == 0 || errno == EPERM
    }

    /// Every process currently registered under the Workbench bundle id,
    /// including instances launched from other worktrees.
    static func runningWorkbenchProcessIdentifiers() -> [pid_t] {
        #if canImport(AppKit)
            return NSRunningApplication.runningApplications(
                withBundleIdentifier: workbenchBundleIdentifier
            ).map(\.processIdentifier)
        #else
            return []
        #endif
    }

    @MainActor
    func dismissCatalogReview(in app: XCUIApplication) {
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
    func waitForNonexistence(
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

    @MainActor
    func scrollToHittable(
        _ element: XCUIElement,
        in app: XCUIApplication
    ) -> Bool {
        guard !element.isHittable else {
            return true
        }
        // Scope the scroll view lookup to the window that actually contains
        // `element`. A previously dismissed sheet (e.g. the account
        // registration window) can still report its own scroll view to an
        // app-wide, unscoped `app.scrollViews.firstMatch` query for a brief
        // window while it finishes tearing down, even after the specific
        // control we waited on has already left the accessibility tree.
        // Swiping that stale scroll view has no effect on the still-open
        // review sheet and would silently spin without ever revealing
        // `element`, so anchor the search to element's own window instead of
        // relying on window ordering.
        let scope = containingWindow(of: element, in: app) ?? app
        let scrollView = scope.scrollViews.firstMatch
        guard scrollView.waitForExistence(timeout: 2) else {
            return false
        }

        // The permission sheet's window is only given `idealHeight: 720`;
        // its floor is `minHeight: 560`. A fixed swipe count tuned against
        // one developer's local display — where the sheet renders near its
        // ideal size and most of the capability list is visible without
        // scrolling — does not generalize: CI runs headless against its own
        // virtual display, which can size the sheet down toward its minimum
        // height and reveal far less of the list per swipe, so a domain
        // that never needed scrolling locally can need several swipes in
        // CI. Loop with a generous, geometry-independent attempt budget
        // instead of a fixed one tuned to a single screen.
        //
        // Swiping is intentionally one-directional (always up, the
        // direction that reveals later rows). An earlier version of this
        // loop tried to "correct" an overshoot by swiping back down
        // whenever the target briefly scrolled out of the visible area
        // above the scroll view. In practice that made things worse: for a
        // row sitting exactly at the end of the scrollable content (e.g.
        // the last capability in the list), alternating swipe directions
        // could make the row's accessibility element flicker in and out of
        // existence entirely and, eventually, made the scroll view itself
        // stop resolving in the accessibility snapshot
        // ("Failed to get matching snapshot ... ScrollView"), reproduced
        // locally. A single scroll direction does not have that failure
        // mode.
        //
        // "Revealed" requires the target's full frame inside the scroll
        // view's visible bounds (see `isFullyRevealed`), not merely
        // `isHittable`: XCUITest can mark a row hittable a frame or two
        // before it has fully crossed the scroll view's clip boundary,
        // which was enough for the old check to stop scrolling but not
        // enough for a subsequent click to reliably open its popup menu —
        // exactly the residual flakiness a previous pass through this test
        // flagged for the last row in the list.
        //
        // Progress is tracked by the target's vertical offset from the
        // scroll view's center. Once swiping stops moving that offset
        // (the scroll view has reached the end of its content — the
        // saturation point), further swipes cannot help, so stop and
        // report a diagnostic instead of silently spinning to the attempt
        // ceiling.
        let maxAttempts = 20
        var consecutiveStalls = 0
        var previousOffset: CGFloat?

        for attempt in 0 ..< maxAttempts {
            scrollView.swipeUp()
            usleep(250_000)

            if isFullyRevealed(element, in: scrollView),
                waitForStableFrame(element, timeout: 2)
            {
                return true
            }

            let scrollFrame = scrollView.frame
            let elementFrame = element.frame
            let offset = abs(elementFrame.midY - scrollFrame.midY)
            if let previousOffset, abs(offset - previousOffset) < 1 {
                consecutiveStalls += 1
            } else {
                consecutiveStalls = 0
            }
            previousOffset = offset

            if consecutiveStalls >= 3 {
                NSLog(
                    "scrollToHittable: giving up on "
                        + "\(element.identifier) after \(attempt + 1) "
                        + "attempt(s) — the scroll view stopped moving it "
                        + "any further (reached the end of its content) "
                        + "without fully revealing it. isHittable="
                        + "\(element.isHittable) scrollView=\(scrollFrame) "
                        + "element=\(elementFrame)"
                )
                return false
            }
        }

        NSLog(
            "scrollToHittable: exhausted \(maxAttempts) attempts revealing "
                + "\(element.identifier). isHittable=\(element.isHittable) "
                + "Last scrollView=\(scrollView.frame) "
                + "element=\(element.frame)"
        )
        return false
    }

    /// Whether `element`'s full frame sits inside `scrollView`'s visible
    /// bounds, not merely at its edge. XCUITest can mark a row `isHittable`
    /// a frame or two before it has fully crossed the scroll view's clip
    /// boundary — enough to stop scrolling but not enough for a subsequent
    /// click to land reliably on it.
    @MainActor
    func isFullyRevealed(
        _ element: XCUIElement,
        in scrollView: XCUIElement
    ) -> Bool {
        guard element.exists, element.isHittable else {
            return false
        }
        let margin: CGFloat = 2
        let visibleBounds = scrollView.frame.insetBy(dx: 0, dy: margin)
        return visibleBounds.contains(element.frame)
    }

    /// `swipeUp()` requests a fast (flinged) scroll, which hands off to
    /// AppKit's own momentum/deceleration animation. That animation runs on
    /// the window server, not the app's run loop, so XCUITest's automatic
    /// "wait for app to idle" step completes before the scrolled content has
    /// actually come to rest. Interacting with a menu-style control while its
    /// row is still drifting underneath the pointer can open a popup menu
    /// that never stabilizes in the accessibility tree before it is
    /// dismissed by the continuing scroll. Waiting for the element's frame
    /// to be identical across two consecutive samples confirms the scroll
    /// has actually settled before the caller clicks it.
    @MainActor
    func waitForStableFrame(
        _ element: XCUIElement,
        timeout: TimeInterval
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        var previousFrame: CGRect?
        while Date() < deadline {
            guard element.isHittable else {
                previousFrame = nil
                usleep(100_000)
                continue
            }
            let currentFrame = element.frame
            if let previousFrame, previousFrame == currentFrame {
                return true
            }
            previousFrame = currentFrame
            usleep(150_000)
        }
        return false
    }

    @MainActor
    func containingWindow(
        of element: XCUIElement,
        in app: XCUIApplication
    ) -> XCUIElement? {
        let identifier = element.identifier
        guard !identifier.isEmpty else {
            return nil
        }
        return app.windows.allElementsBoundByIndex.first { window in
            window.descendants(matching: .any)[identifier].exists
        }
    }

    /// Grants one capability through the review sheet's per-capability
    /// switch. The sheet exposes a single switch per domain rather than a
    /// scope menu: which scope a grant actually uses is Rust's
    /// `recommendedDecision`, not something this suite picks.
    @MainActor
    @discardableResult
    func grantPermission(
        domain: String,
        in app: XCUIApplication,
        message: String? = nil
    ) -> Bool {
        let toggle = app.descendants(matching: .any)[
            "permission-toggle-\(domain)"
        ]
        guard toggle.waitForExistence(timeout: 10) else {
            XCTFail(
                message
                    ?? "The \(domain) permission switch must exist in the native review"
            )
            return false
        }
        let anchor = app.buttons["permission-scroll-to-\(domain)"]
        if anchor.waitForExistence(timeout: 2) {
            anchor.click()
            _ = waitForStableFrame(toggle, timeout: 5)
        } else {
            _ = scrollToHittable(toggle, in: app)
        }
        if (toggle.value as? String) == "1" {
            return true
        }
        toggle.click()
        return true
    }
}
