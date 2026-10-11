import MediaFlowKit
import SwiftUI

/// 六屏状态容器（薄 Swift Facade：业务全部经 MediaFlowIosBridge 进 KMP SDK）。
@MainActor
final class AppModel: ObservableObject {
    @Published var loggedIn = false
    @Published var loginBusy = false
    @Published var loginError: String?
    @Published var baseUrl = "http://127.0.0.1:8642"
    @Published var username = "admin"
    @Published var password = ""

    @Published var path: [Screen] = []
    @Published var tab: Tab = .wall

    @Published var wallItems: [WallItem] = []
    @Published var wallHasMore = false
    @Published var wallMode: PageVisualMode = .loading(stale: false)
    private var wallCursor: String?

    @Published var detail: DetailUi?
    @Published var detailLoading = false

    @Published var cases: [ReviewCaseUi] = []
    @Published var casesMode: PageVisualMode = .loading(stale: false)

    @Published var candidates: [CandidateUi] = []
    @Published var candidatesSearching = false
    @Published var candidateQuery = ""
    @Published var candidateMediaType = "movie"
    @Published var submitting = false
    @Published var receipt: ReceiptUi?
    @Published var confirmError: String?

    @Published var tasks: [TaskUi] = []
    @Published var tasksMode: PageVisualMode = .loading(stale: false)
    @Published var cancellingIds: Set<String> = []

    private let bridge = MediaFlowIosBridge()

    init() {
        // SSE 事件 → 对应 Tab 的权威刷新（与 Android ProjectionRefresher 同语义）。
        bridge.setEventHandler { [weak self] eventType in
            Task { @MainActor in
                guard let self else { return }
                switch eventType {
                case "catalog.media-changed", "stream.gap":
                    await self.reloadWall()
                case "review-case.updated", "processing-task.identification-decided":
                    await self.reloadCases()
                case "processing-task.state-changed", "task.state-changed":
                    await self.reloadTasks()
                default:
                    break
                }
            }
        }
    }

    func login() {
        loginBusy = true
        loginError = nil
        let url = baseUrl
        let name = username
        let secret = password
        Task { @MainActor in
            do {
                let outcome = try await bridge.login(baseUrl: url, name: name, password: secret)
                if outcome.success {
                    loggedIn = true
                } else {
                    loginError = outcome.message ?? "登录失败"
                }
            } catch {
                loginError = "登录失败"
            }
            loginBusy = false
        }
    }

    func reloadWall() async {
        do {
            applyWall(try await bridge.wallPage(cursor: nil))
        } catch {
            applyWall(WallOutcome(page: nil, error: "加载失败"))
        }
    }

    func loadMoreWall() {
        let cursor = wallCursor
        guard let cursor else { return }
        Task { @MainActor in
            do {
                applyWall(try await bridge.wallPage(cursor: cursor), append: true)
            } catch {
                applyWall(WallOutcome(page: nil, error: "加载失败"), append: true)
            }
        }
    }

    private func applyWall(_ outcome: WallOutcome, append: Bool = false) {
        if let page = outcome.page {
            wallItems = append ? wallItems + page.items : page.items
            wallCursor = page.nextCursor
            wallHasMore = page.nextCursor != nil
            wallMode = wallItems.isEmpty ? .empty : .content
        } else {
            let message = outcome.error ?? "加载失败"
            wallMode = message.contains("网络") || message.contains("离线")
                ? .offline(stale: !wallItems.isEmpty)
                : .error(message: message, stale: !wallItems.isEmpty)
        }
    }

    func loadDetail(_ screen: Screen) async {
        guard case let .detail(id, _) = screen else { return }
        detailLoading = true
        detail = nil
        do {
            detail = try await bridge.detail(mediaItemId: id).detail
        } catch {
            detail = nil
        }
        detailLoading = false
    }

    func reloadCases() async {
        do {
            let result = try await bridge.reviewCases()
            if result.error == nil {
                cases = result.cases
                casesMode = result.cases.isEmpty ? .empty : .content
            } else {
                casesMode = .error(message: result.error ?? "加载失败", stale: !cases.isEmpty)
            }
        } catch {
            casesMode = .error(message: "加载失败", stale: !cases.isEmpty)
        }
    }

    func searchCandidates(_ screen: Screen) async {
        guard case let .confirm(caseId, _) = screen else { return }
        candidatesSearching = true
        candidates = []
        do {
            let result = try await bridge.candidates(
                reviewCaseId: caseId,
                query: candidateQuery,
                mediaType: candidateMediaType
            )
            candidates = result.candidates
        } catch {
            candidates = []
        }
        candidatesSearching = false
    }

    func submitCandidate(_ screen: Screen, candidate: CandidateUi) async {
        guard case let .confirm(caseId, _) = screen,
              let item = cases.first(where: { $0.id == caseId }) else { return }
        submitting = true
        confirmError = nil
        do {
            let result = try await bridge.submitCandidate(
                reviewCaseId: caseId,
                caseVersion: item.version,
                providerId: candidate.providerId,
                mediaTypeRaw: candidate.mediaTypeRaw
            )
            receipt = result.receipt
            confirmError = result.error
        } catch {
            confirmError = "提交失败"
        }
        submitting = false
    }

    func submitGeneric(_ screen: Screen) async {
        guard case let .confirm(caseId, title) = screen,
              let item = cases.first(where: { $0.id == caseId }) else { return }
        submitting = true
        confirmError = nil
        do {
            let result = try await bridge.submitGeneric(
                reviewCaseId: caseId,
                caseVersion: item.version,
                displayTitle: title
            )
            receipt = result.receipt
            confirmError = result.error
        } catch {
            confirmError = "提交失败"
        }
        submitting = false
    }

    func reloadTasks() async {
        do {
            let result = try await bridge.tasks()
            if result.error == nil {
                tasks = result.tasks
                tasksMode = result.tasks.isEmpty ? .empty : .content
            } else {
                tasksMode = .error(message: result.error ?? "加载失败", stale: !tasks.isEmpty)
            }
        } catch {
            tasksMode = .error(message: "加载失败", stale: !tasks.isEmpty)
        }
    }

    func cancelTask(_ taskId: String) async {
        cancellingIds.insert(taskId)
        _ = try? await bridge.cancelTask(taskId: taskId)
        cancellingIds.remove(taskId)
        await reloadTasks()
    }
}
