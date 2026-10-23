import XCTest

final class ActionTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
        // Disable XCTest's diagnostic media as well as omitting all screenshot APIs.
        // Preferences are scoped to the generated test runner application.
        UserDefaults.standard.set(true, forKey: "DisableDiagnosticScreenshots")
        UserDefaults.standard.set(true, forKey: "DisableDiagnosticScreenRecordings")
    }

    private func reveal(_ element: XCUIElement, in app: XCUIApplication) {
        let top = app.frame.minY + 160
        let bottom = app.frame.maxY - 120
        func scroll(up: Bool) {
            let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: up ? 0.65 : 0.45))
            let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: up ? 0.45 : 0.65))
            start.press(forDuration: 0.05, thenDragTo: end, withVelocity: .slow, thenHoldForDuration: 0.1)
        }
        func reachable() -> Bool {
            element.exists && element.isHittable && element.frame.midY > top && element.frame.midY < bottom
        }
        for _ in 0..<14 {
            if reachable() { return }
            if element.exists && element.frame.midY <= top {
                scroll(up: false)
            } else {
                scroll(up: true)
            }
        }
        for _ in 0..<14 {
            if reachable() { return }
            scroll(up: false)
        }
        print(app.debugDescription)
        XCTAssertTrue(reachable(), "Expected control inside the visible safe content area")
    }

    func testBusyKeepsNameAndFailureAllowsRetry() {
        let app = XCUIApplication()
        app.launch()
        let reserve = app.buttons["Reserve seat"]
        reveal(reserve, in: app)
        reserve.tap()
        XCTAssertTrue(reserve.exists)
        XCTAssertFalse(reserve.isEnabled)
        let failure = app.buttons["Simulate failure"]
        reveal(failure, in: app)
        failure.tap()
        reveal(reserve, in: app)
        expectation(for: NSPredicate(format: "enabled == true"), evaluatedWith: reserve)
        waitForExpectations(timeout: 4)
        reserve.tap()
        let attempts = app.descendants(matching: .any)["attempts"]
        XCTAssertTrue(attempts.waitForExistence(timeout: 3))
        XCTAssertEqual(attempts.label, "Attempts: 2")
        XCTAssertFalse(app.buttons["Unavailable"].isEnabled)
    }

    func testModalBusyFailureAndExplicitDiscardRetainDraft() {
        let app = XCUIApplication()
        app.launch()
        let edit = app.buttons["Edit modal"]
        reveal(edit, in: app)
        edit.tap()
        let save = app.buttons["Save preview"]
        save.tap()
        XCTAssertEqual(app.descendants(matching: .any)["modal-error"].label, "Enter a display name.")
        let name = app.textFields["modal-name"]
        name.tap()
        name.typeText("Local draft\n")
        save.tap()
        XCTAssertFalse(save.isEnabled)
        XCTAssertFalse(app.buttons["Cancel"].isEnabled)
        let failure = app.buttons["Fail save"]
        reveal(failure, in: app)
        failure.tap()
        reveal(name, in: app)
        XCTAssertEqual(name.value as? String, "Local draft")
        XCTAssertEqual(app.descendants(matching: .any)["modal-error"].label, "Save failed. Your draft is retained.")
        XCTAssertTrue(save.isEnabled)
        app.buttons["Cancel"].tap()
        app.alerts.buttons["Continue editing"].tap()
        XCTAssertEqual(name.value as? String, "Local draft")
        app.buttons["Cancel"].tap()
        app.alerts.buttons["Discard draft"].tap()
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.textFields["modal-name"])
        waitForExpectations(timeout: 5)
    }

    func testContentRowNavigatesToCompleteTitleAndReturns() {
        let app = XCUIApplication()
        app.launch()
        let title = "Accessible interfaces: a longer conference session title"
        let row = app.buttons.containing(NSPredicate(format: "label CONTAINS %@", title)).firstMatch
        reveal(row, in: app)
        row.tap()
        XCTAssertTrue(app.navigationBars["Complete title"].exists)
        XCTAssertTrue(app.staticTexts[title].exists)
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.navigationBars["Foundation studio"].exists)
    }
}
