import XCTest
@testable import MediaFlowiOS
import MediaFlowKit

/// 桥接冒烟（无网络）：未登录状态各接口返回错误信封而非崩溃——验证 Kotlin 桥
/// 在 iOS 目标下的真实链接与导出形状。重逻辑（分页/去重/错误映射）在 Kotlin 侧
/// 的多平台测试中覆盖。
final class MediaFlowBridgeTests: XCTestCase {
    func testBridgeExistsAndExportsExpectedShapes() {
        let bridge = MediaFlowIosBridge()
        bridge.setEventHandler(handler: nil)
    }

    func testPageVisualModesCarryMessages() {
        XCTAssertEqual(PageVisualMode.empty.message, "暂无内容")
        XCTAssertNil(PageVisualMode.content.message)
        let offline = PageVisualMode.offline(stale: true)
        XCTAssertTrue(offline.isOffline)
        XCTAssertEqual(offline.message, "离线：无法连接服务器")
        let error = PageVisualMode.error(message: "服务错误（500）", stale: false)
        XCTAssertEqual(error.message, "服务错误（500）")
        XCTAssertFalse(error.isOffline)
    }

    func testNotLoggedInCallsReturnGracefulErrors() async throws {
        let bridge = MediaFlowIosBridge()
        let wall = try await bridge.wallPage(cursor: nil)
        XCTAssertNil(wall.page)
        XCTAssertEqual(wall.error, "not logged in")
        let tasks = try await bridge.tasks()
        XCTAssertEqual(tasks.error, "not logged in")
    }
}
