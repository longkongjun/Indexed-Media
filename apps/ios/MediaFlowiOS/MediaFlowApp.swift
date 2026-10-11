import MediaFlowKit
import SwiftUI

@main
struct MediaFlowApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
        }
    }
}

/// 页面状态语义（与 Android PageVisual / Web TaskCenterState 同一套五态 + stale）。
enum PageVisualMode: Equatable {
    case loading(stale: Bool)
    case content
    case empty
    case error(message: String, stale: Bool)
    case offline(stale: Bool)

    var message: String? {
        switch self {
        case .loading: nil
        case .content: nil
        case .empty: "暂无内容"
        case let .error(message, _): message
        case .offline: "离线：无法连接服务器"
        }
    }
}

enum Tab: Hashable { case wall, review, tasks }

enum Screen: Hashable {
    case login
    case tab(Tab)
    case detail(id: String, title: String)
    case confirm(caseId: String, title: String)
}

/// 根路由与六屏导航（与 Android AppRoot 对齐：Tab + push 栈）。
struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        NavigationStack(path: $model.path) {
            if model.loggedIn {
                TabRootView()
            } else {
                LoginView()
            }
        }
    }
}

struct TabRootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        TabView(selection: $model.tab) {
            WallView().tabItem { Label("媒体库", systemImage: "square.stack") }.tag(Tab.wall)
            ReviewQueueView().tabItem { Label("确认", systemImage: "checkmark.circle") }.tag(Tab.review)
            TasksView().tabItem { Label("任务", systemImage: "list.bullet") }.tag(Tab.tasks)
        }
    }
}

struct LoginView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Form {
            Section {
                TextField("服务器地址", text: $model.baseUrl)
                    .keyboardType(.URL)
                    .textInputAutocapitalization(.never)
                TextField("管理员", text: $model.username)
                SecureField("密码", text: $model.password)
            }
            if let error = model.loginError {
                Text(error).foregroundStyle(.red)
            }
            Button {
                model.login()
            } label: {
                if model.loginBusy {
                    ProgressView()
                } else {
                    Text("登录").frame(maxWidth: .infinity)
                }
            }
            .disabled(model.loginBusy || model.password.isEmpty)
        }
        .navigationTitle("MediaFlow")
    }
}

struct WallView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        List {
            ForEach(model.wallItems, id: \.id) { item in
                Button {
                    model.path.append(Screen.detail(id: item.id, title: item.title))
                } label: {
                    HStack {
                        Text(item.hasArtwork ? "▣" : "□").font(.title2)
                        VStack(alignment: .leading) {
                            Text(item.title).font(.headline)
                            Text("\(item.typeLabel) · \(item.yearLabel) · \(item.statusLabel)")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if model.wallHasMore {
                Button("加载更多") { model.loadMoreWall() }
            }
        }
        .refreshable { await model.reloadWall() }
        .overlay { if model.wallMode == .loading(stale: false) { ProgressView("加载中…") } }
        .navigationTitle("媒体库")
        .task { await model.reloadWall() }
        .overlay(alignment: .bottom) { StateBanner(mode: model.wallMode) }
    }
}

func screenTitle(_ screen: Screen) -> String {
    switch screen {
    case let .detail(_, title): title
    case let .confirm(_, title): title
    default: ""
    }
}

struct DetailView: View {
    @EnvironmentObject private var model: AppModel
    let screen: Screen

    var body: some View {
        List {
            if let detail = model.detail {
                Section(detail.title) {
                    Text(detail.subtitle)
                    if let url = detail.playbackUrl, let target = URL(string: url) {
                        Link("外部播放（Jellyfin）", destination: target)
                    } else {
                        Label("外部播放不可用（Jellyfin 未配置或不健康）", systemImage: "exclamationmark.circle")
                            .foregroundStyle(.secondary)
                    }
                }
                if !detail.metadataLines.isEmpty {
                    Section("元数据") { ForEach(detail.metadataLines, id: \.self) { Text($0) } }
                }
                if !detail.versionLines.isEmpty {
                    Section("版本") { ForEach(detail.versionLines, id: \.self) { Text($0) } }
                }
                if !detail.childTitles.isEmpty {
                    Section("分集/子项") { ForEach(detail.childTitles, id: \.self) { Text($0) } }
                }
            }
        }
        .navigationTitle(screenTitle(screen))
        .overlay { if model.detailLoading { ProgressView("加载中…") } }
        .task(id: screen) { await model.loadDetail(screen) }
    }
}

struct ReviewQueueView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        List {
            ForEach(model.cases, id: \.id) { item in
                Button {
                    model.path.append(Screen.confirm(caseId: item.id, title: item.displayTitle))
                } label: {
                    VStack(alignment: .leading) {
                        Text(item.displayTitle).font(.headline)
                        Text("级别 \(item.levelLabel) · 版本 \(item.version)")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .refreshable { await model.reloadCases() }
        .navigationTitle("人工确认")
        .task { await model.reloadCases() }
        .overlay(alignment: .bottom) { StateBanner(mode: model.casesMode) }
    }
}

struct ConfirmView: View {
    @EnvironmentObject private var model: AppModel
    let screen: Screen

    var body: some View {
        Form {
            Section(screenTitle(screen)) {
                if let receipt = model.receipt {
                    Label("已提交（\(receipt.stateLabel)）· 案件版本 \(receipt.caseVersion)", systemImage: "checkmark.circle")
                        .foregroundStyle(.green)
                } else {
                    Picker("类型", selection: $model.candidateMediaType) {
                        Text("电影").tag("movie")
                        Text("剧集").tag("tv")
                    }
                    HStack {
                        TextField("搜索标题", text: $model.candidateQuery)
                        Button("搜索") { Task { await model.searchCandidates(screen) } }
                            .disabled(model.candidateQuery.isEmpty || model.candidatesSearching)
                    }
                }
            }
            if model.candidatesSearching {
                Section { ProgressView() }
            } else if !model.candidates.isEmpty {
                Section("候选") {
                    ForEach(Array(model.candidates.enumerated()), id: \.offset) { index, candidate in
                        Button {
                            Task { await model.submitCandidate(screen, candidate: candidate) }
                        } label: {
                            VStack(alignment: .leading) {
                                Text(candidate.title).font(.headline)
                                Text(candidate.subtitle).font(.caption).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
            if model.receipt == nil {
                Section {
                    Button("标记为通用视频（不做影片识别）") { Task { await model.submitGeneric(screen) } }
                        .disabled(model.submitting)
                }
            }
            if let error = model.confirmError {
                Text(error).foregroundStyle(.red)
            }
        }
        .navigationTitle("确认识别")
    }
}

struct TasksView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        List {
            ForEach(model.tasks, id: \.id) { taskRow in
                HStack {
                    VStack(alignment: .leading) {
                        Text(taskRow.pathLabel).font(.headline).lineLimit(2)
                        Text(taskRow.statusLabel).font(.caption).foregroundStyle(.secondary)
                    }
                    Spacer()
                    if taskRow.cancellable {
                        Button(model.cancellingIds.contains(taskRow.id) ? "取消中…" : "取消") {
                            Task { await model.cancelTask(taskRow.id) }
                        }
                        .disabled(model.cancellingIds.contains(taskRow.id))
                    }
                }
            }
        }
        .refreshable { await model.reloadTasks() }
        .navigationTitle("任务中心")
        .task { await model.reloadTasks() }
        .overlay(alignment: .bottom) { StateBanner(mode: model.tasksMode) }
    }
}

/// 五态语义横幅（error/offline + stale 提示 + 重试）。
struct StateBanner: View {
    let mode: PageVisualMode

    var body: some View {
        switch mode {
        case .loading, .content, .empty:
            EmptyView()
        case let .error(message, stale):
            banner(text: message + (stale ? "（内容可能已过期）" : ""))
        case let .offline(stale):
            banner(text: "离线：无法连接服务器" + (stale ? "（内容可能已过期）" : ""))
        }
    }
}

private func banner(text: String) -> some View {
    HStack {
        Text(text).font(.footnote)
        Spacer()
    }
    .padding(8)
    .background(.thinMaterial)
}

extension PageVisualMode {
    var isOffline: Bool {
        if case .offline = self { return true }
        return false
    }
}
