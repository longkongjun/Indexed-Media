import XCTest

final class ActionTests: XCTestCase {
    func testBusyKeepsNameAndFailureAllowsRetry() {
        let app = XCUIApplication()
        app.launch()
        let reserve = app.buttons["Reserve seat"]
        reserve.tap()
        XCTAssertTrue(reserve.exists)
        XCTAssertFalse(reserve.isEnabled)
        XCTAssertEqual(app.staticTexts["attempts"].label, "Attempts: 1")
        app.buttons["Simulate failure"].tap()
        XCTAssertTrue(reserve.isEnabled)
        reserve.tap()
        XCTAssertEqual(app.staticTexts["attempts"].label, "Attempts: 2")
    }
}
