import MediaFlowKit
import SwiftUI

/// 六屏状态容器（薄 Swift Facade：业务全部经 MediaFlowIosBridge 进 KMP SDK）。
@MainActor
final class AppModel: ObservableObject {
    @Published var loggedIn = false
    @Published var loginBusy = false
    @Published var loginError: String?
    /// 上次登录地址持久化（UserDefaults），冷启动恢复与登录页默认值共用。
    @Published var baseUrl = UserDefaults.standard.string(forKey: "mediaflow.base-url") ?? "http://127.0.0.1:8642"
    @Published var username = "admin"
    @Published var password = ""
    /// 登录页模式：配对码（Web 签发的一次性码）或管理员密码，统一产设备令牌。
    @Published var pairingMode = true
    @Published var pairingCode = ""

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
        // 任意屏 401：桥内已清 Keychain，统一回登录页。
        bridge.setCredentialExpiredHandler { [weak self] in
            Task { @MainActor in
                self?.loggedIn = false
                self?.path = []
            }
        }
        // 冷启动恢复：Keychain 有设备令牌则直接进入（首个请求自然校验）。
        Task { @MainActor in
            if let outcome = try? await bridge.restore(baseUrl: baseUrl), outcome.success {
                loggedIn = true
            }
        }
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

    /// 按登录页模式发起 enrollment：配对码或管理员凭据，统一产设备令牌。
    func login() {
        loginBusy = true
        loginError = nil
        let url = baseUrl
        let mode = pairingMode
        let code = pairingCode
        let name = username
        let secret = password
        Task { @MainActor in
            do {
                let outcome: Outcome
                if mode {
                    outcome = try await bridge.pair(baseUrl: url, pairingCode: code)
                } else {
                    outcome = try await bridge.login(baseUrl: url, name: name, password: secret)
                }
                if outcome.success {
                    UserDefaults.standard.set(url, forKey: "mediaflow.base-url")
                    loggedIn = true
                } else {
                    loginError = outcome.message ?? (mode ? "配对失败" : "登录失败")
                }
            } catch {
                loginError = mode ? "配对失败" : "登录失败"
            }
            loginBusy = false
        }
    }

    /// 登出：吊销设备令牌并清 Keychain，回登录页。
    func logout() {
        Task { @MainActor in
            _ = try? await bridge.logout()
            loggedIn = false
            path = []
            tab = .wall
            wallItems = []
            cases = []
            tasks = []
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

extension AppModel {
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
}
