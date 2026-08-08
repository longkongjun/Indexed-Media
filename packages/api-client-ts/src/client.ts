import type { components } from "./generated/mediaflow.js";

/** MediaFlow HTTP 失败时返回的结构化白名单错误信封。 */
export type ApiErrorBody = components["schemas"]["ApiErrorBody"];
/** 实例的初始化就绪状态和 API 版本投影。 */
export type BootstrapStatusResponse = components["schemas"]["BootstrapStatusResponse"];
/** 用于创建唯一管理员账户的一次性初始化凭据。 */
export type BootstrapRequest = components["schemas"]["BootstrapRequest"];
/** 初始化变更成功后返回的管理员投影。 */
export type BootstrapResponse = components["schemas"]["BootstrapResponse"];
/** 用于创建浏览器会话而提交的管理员凭据。 */
export type LoginRequest = components["schemas"]["LoginRequest"];
/** 会话返回的已认证账户投影和内存中的 CSRF 令牌。 */
export type SessionResponse = components["schemas"]["SessionResponse"];
/** 管理员可见的有界部署能力根目录列表。 */
export type DeploymentRootList = components["schemas"]["DeploymentRootList"];
/** 用于不持久化收件箱预检的能力根目录标识符和相对路径。 */
export type InboxDirectoryPreflightRequest = components["schemas"]["InboxDirectoryPreflightRequest"];
/** 收件箱预检返回的规范化路径、可读性和重叠检查结果。 */
export type InboxDirectoryPreflight = components["schemas"]["InboxDirectoryPreflight"];
/** 创建收件箱目录时提交的能力根目录标识符和相对路径。 */
export type CreateInboxDirectoryRequest = components["schemas"]["CreateInboxDirectoryRequest"];
/** 已持久化收件箱目录投影的游标分页。 */
export type InboxDirectoryPage = components["schemas"]["InboxDirectoryPage"];
/** 已持久化收件箱目录的标识、规范化相对路径和最近观测到的健康状态。 */
export type InboxDirectory = components["schemas"]["InboxDirectory"];
/** 持久化扫描任务投影的游标分页。 */
export type ScanTaskPage = components["schemas"]["ScanTaskPage"];
/** 持久化扫描任务状态、恢复标志和已提交的聚合计数。 */
export type ScanTask = components["schemas"]["ScanTask"];
/** 扫描任务已提交文件事实的游标分页。 */
export type DiscoveredFilePage = components["schemas"]["DiscoveredFilePage"];
/** 持久化逐路径扫描错误聚合的游标分页。 */
export type ScanErrorPage = components["schemas"]["ScanErrorPage"];
/** 已验证的第一版任务、下载、审核、媒体目录、健康或流缺口事件信封。 */
export type TaskEventEnvelope = components["schemas"]["TaskEventEnvelope"];
/** 已脱敏的 TMDB 配置与健康投影。 */
export type TmdbIntegration = components["schemas"]["TmdbIntegration"];
/** 用于保存 TMDB 凭据和语言偏好的写入体。 */
export type PutTmdbIntegrationRequest = components["schemas"]["PutTmdbIntegrationRequest"];
/** 仅用于不落盘连接测试的候选 TMDB 凭据。 */
export type TmdbConnectionTestRequest = components["schemas"]["TmdbConnectionTestRequest"];
/** 已脱敏的 TMDB 连接测试结果。 */
export type TmdbConnectionTestResult = components["schemas"]["TmdbConnectionTestResult"];
/** 收件目录持续发现和稳定门禁策略。 */
export type DiscoveryPolicy = components["schemas"]["DiscoveryPolicy"];
/** 替换持续发现策略的写入体。 */
export type PutDiscoveryPolicyRequest = components["schemas"]["PutDiscoveryPolicyRequest"];
/** 单文件持久 ProcessingTask 投影。 */
export type ProcessingTask = components["schemas"]["ProcessingTask"];
/** ProcessingTask 的稳定游标分页。 */
export type ProcessingTaskPage = components["schemas"]["ProcessingTaskPage"];
/** 一个任务的有界证据、候选和决定详情。 */
export type IdentificationDetail = components["schemas"]["IdentificationDetail"];
/** 可执行显式人工决定的识别审核案例投影。 */
export type ReviewCase = components["schemas"]["ReviewCase"];
/** ReviewCase 的稳定游标分页。 */
export type ReviewCasePage = components["schemas"]["ReviewCasePage"];
/** 人工选择、重新匹配或通用视频意图的实际 wire discriminator 联合。 */
export type ReviewDecisionRequest =
  | (Omit<components["schemas"]["SelectProviderCandidateCommand"], "kind"> & { kind: "select-provider-candidate" })
  | (Omit<components["schemas"]["RematchWithHintsCommand"], "kind"> & { kind: "rematch-with-hints" })
  | (Omit<components["schemas"]["SelectGenericVideoCommand"], "kind"> & { kind: "select-generic-video" });
/** 已持久化人工决定的安全回执。 */
export type TaskDecisionReceipt = components["schemas"]["TaskDecisionReceipt"];
/** 交互搜索返回的有界临时候选。 */
export type ReviewCandidatePage = components["schemas"]["ReviewCandidatePage"];
/** 正式本地媒体记录的稳定分页。 */
export type MediaItemPage = components["schemas"]["MediaItemPage"];
/** 一个正式本地媒体记录的有界详情。 */
export type MediaItemDetail = components["schemas"]["MediaItemDetail"];
/** 已脱敏且版本化的下载器连接投影。 */
export type DownloaderConnection = components["schemas"]["DownloaderConnection"];
/** 下载器连接的稳定游标分页。 */
export type DownloaderConnectionPage = components["schemas"]["DownloaderConnectionPage"];
/** 候选下载器配置；用户名和密码只用于当前写请求。 */
export type DownloaderConnectionInput = components["schemas"]["DownloaderConnectionInput"];
/** 不落盘下载器连接测试的脱敏结果。 */
export type DownloaderConnectionTestResult = components["schemas"]["DownloaderConnectionTestResult"];
/** MediaFlow 自有下载任务的脱敏投影。 */
export type DownloadTask = components["schemas"]["DownloadTask"];
/** 下载任务的稳定游标分页。 */
export type DownloadTaskPage = components["schemas"]["DownloadTaskPage"];
/** 接受一个加密保存的手动下载源。 */
export type CreateDownloadTaskRequest = components["schemas"]["CreateDownloadTaskRequest"];
/** 已脱敏且版本化的自动来源投影。 */
export type AutomationSource = components["schemas"]["AutomationSource"];
/** 自动来源的稳定游标分页。 */
export type AutomationSourcePage = components["schemas"]["AutomationSourcePage"];
/** RSS、Webhook 或下载完成映射的严格写入联合。 */
export type AutomationSourceInput =
  | (Omit<components["schemas"]["RssAutomationSourceInput"], "kind"> & { kind: "rss" })
  | (Omit<components["schemas"]["WebhookAutomationSourceInput"], "kind"> & { kind: "webhook" })
  | (Omit<components["schemas"]["DownloadCompletionAutomationSourceInput"], "kind"> & { kind: "download-completion" });
/** 创建来源后返回的脱敏投影或 Webhook 一次性 secret 回执。 */
export type AutomationSourceCreateResult = components["schemas"]["AutomationSourceCreateResult"];
/** 候选来源的不落盘健康测试结果。 */
export type AutomationSourceConnectionTestResult = components["schemas"]["AutomationSourceConnectionTestResult"];
/** Webhook secret 创建或轮换时只返回一次的回执。 */
export type WebhookSecretReceipt = components["schemas"]["WebhookSecretReceipt"];
/** 不含动作载荷、来源秘密或路径的自动化事件投影。 */
export type AutomationEvent = components["schemas"]["AutomationEvent"];
/** 自动化事件的稳定游标分页。 */
export type AutomationEventPage = components["schemas"]["AutomationEventPage"];
/** 本地 Ollama 识别增强器的候选配置。 */
export type IdentificationEnhancerInput = components["schemas"]["IdentificationEnhancerInput"];
/** 已脱敏的本地识别增强器配置与健康。 */
export type IdentificationEnhancer = components["schemas"]["IdentificationEnhancer"];
/** 候选本地增强器的不落盘连接结果。 */
export type IdentificationEnhancerConnectionTestResult = components["schemas"]["IdentificationEnhancerConnectionTestResult"];
/** 版本化整理目标、固定 profile 与有界规则的安全投影。 */
export type OrganizationTarget = components["schemas"]["OrganizationTarget"];
/** 整理目标的稳定游标分页。 */
export type OrganizationTargetPage = components["schemas"]["OrganizationTargetPage"];
/** 创建或替换整理目标聚合时提交的根内配置。 */
export type OrganizationTargetInput = components["schemas"]["OrganizationTargetInput"];
/** 不落盘目标检查所使用的能力根与相对路径。 */
export type OrganizationTargetPreflightRequest = components["schemas"]["OrganizationTargetPreflightRequest"];
/** 不产生目录副作用的目标能力与重叠检查结果。 */
export type OrganizationTargetPreflight = components["schemas"]["OrganizationTargetPreflight"];
/** 一个 ProcessingTask 的计划、journal、本地结果和服务端允许动作。 */
export type ProcessingTaskOrganization = components["schemas"]["ProcessingTaskOrganization"];

/** ProcessingTask 中心的稳定服务端筛选。 */
export interface ProcessingTaskListOptions {
  cursor?: string;
  view?: "pending" | "running" | "all" | "completed";
  stage?: ProcessingTask["stage"];
  status?: ProcessingTask["status"] | "partial-success" | "completed" | "failed";
  inboxDirectoryId?: string;
  query?: string;
}

/** ReviewCase 上下文内的有界临时候选搜索。 */
export interface ReviewCandidateSearchOptions {
  query: string;
  mediaType: "movie" | "tv";
  locale: string;
  limit?: number;
}

/** 正式 Catalog 列表的稳定服务端筛选。 */
export interface MediaItemListOptions {
  cursor?: string;
  type?: "movie" | "series" | "generic-video";
  libraryId?: string;
  localStatus?: "complete" | "partial";
  query?: string;
}

/** ReviewCase 列表的可选稳定筛选条件。 */
export interface ReviewCaseListOptions {
  cursor?: string;
  level?: ReviewCase["level"];
  inboxDirectoryId?: string;
  updatedBefore?: string;
}

/** 下载任务列表的稳定服务端筛选。 */
export interface DownloadTaskListOptions {
  cursor?: string;
  connectionId?: string;
  status?: DownloadTask["status"];
  query?: string;
}

/** 自动化事件列表的稳定服务端筛选。 */
export interface AutomationEventListOptions {
  cursor?: string;
  sourceId?: string;
  status?: AutomationEvent["status"];
  action?: AutomationEvent["action"];
}

/**
 * MediaFlow HTTP 响应不成功时抛出的错误。
 *
 * @remarks 仅当响应包含有效的白名单 API 错误信封时才会提供 `body`；格式错误或非 JSON 的响应体会被丢弃，
 * 以免调用方将任意响应内容误认为可信详情。
 */
export class MediaFlowApiError extends Error {
  /**
   * 根据已验证的响应创建 API 错误。
   *
   * @param status - HTTP 响应状态。
   * @param body - 若响应返回，则为已验证的 MediaFlow 错误信封。
   */
  constructor(readonly status: number, readonly body: ApiErrorBody | undefined) {
    super(body?.error.message ?? `MediaFlow API request failed with status ${status}`);
    this.name = "MediaFlowApiError";
  }
}

/**
 * MediaFlow v1 身份、收件箱、扫描、持续发现、单文件识别、审核管理、正式媒体目录和下载器管理 API 的轻量传输契约。
 *
 * 读写方法均解析为从 OpenAPI 派生的投影；HTTP 失败时以 `MediaFlowApiError` 拒绝，传输失败时以底层 fetch
 * 错误拒绝。若 Core 提交变更后响应可能丢失，写入方必须提供幂等键。
 */
export interface MediaFlowClient {
  /** @returns 是否需要初始化以及契约版本。 */
  getBootstrapStatus(): Promise<BootstrapStatusResponse>;
  /** @param body - 一次性密钥和新管理员凭据。@returns 已创建的账户投影。 */
  bootstrap(body: BootstrapRequest): Promise<BootstrapResponse>;
  /** @param body - 管理员凭据。@returns 账户和 CSRF 会话投影。 */
  createSession(body: LoginRequest): Promise<SessionResponse>;
  /** @returns 当前账户和刷新后的 CSRF 会话投影。 */
  getSession(): Promise<SessionResponse>;
  /** 删除服务端会话，并在成功收到空响应后完成。 */
  deleteSession(): Promise<void>;
  /** @returns 本地运维人员配置的部署根目录。 */
  listDeploymentRoots(): Promise<DeploymentRootList>;
  /** @param body - 待检查但不持久化的根目录和相对路径。@returns 规范化和安全检查结果。 */
  preflightInboxDirectory(body: InboxDirectoryPreflightRequest): Promise<InboxDirectoryPreflight>;
  /** @param cursor - 不透明的下一页游标。@returns 一页收件箱目录。 */
  listInboxDirectories(cursor?: string): Promise<InboxDirectoryPage>;
  /** @param body - 用于权威验证的根目录相对路径。@returns 已持久化的收件箱目录。 */
  createInboxDirectory(body: CreateInboxDirectoryRequest): Promise<InboxDirectory>;
  /** @param id - 收件箱目录 UUID。@returns 其最新持久化投影。 */
  getInboxDirectory(id: string): Promise<InboxDirectory>;
  /** @param inboxDirectoryId - 源目录 UUID。@param idempotencyKey - 用于安全重放的稳定键。@returns 已接受的任务。 */
  createScanTask(inboxDirectoryId: string, idempotencyKey: string): Promise<ScanTask>;
  /** @param cursor - 不透明的下一页游标。@returns 一页扫描任务。 */
  listScanTasks(cursor?: string): Promise<ScanTaskPage>;
  /** @param id - 扫描任务 UUID。@returns 其最新持久化投影。 */
  getScanTask(id: string): Promise<ScanTask>;
  /** @param id - 可重试任务 UUID。@param idempotencyKey - 用于安全重放的稳定键。@returns 已接受的重试投影。 */
  retryScanTask(id: string, idempotencyKey: string): Promise<ScanTask>;
  /** @param id - 活动任务 UUID。@param idempotencyKey - 用于安全重放的稳定键。@returns 已接受的取消投影。 */
  cancelScanTask(id: string, idempotencyKey: string): Promise<ScanTask>;
  /** @param id - 扫描任务 UUID。@param cursor - 不透明的下一页游标。@returns 一页已提交的文件事实。 */
  listScanTaskFiles(id: string, cursor?: string): Promise<DiscoveredFilePage>;
  /** @param id - 扫描任务 UUID。@param cursor - 不透明的下一页游标。@returns 一页错误聚合。 */
  listScanTaskErrors(id: string, cursor?: string): Promise<ScanErrorPage>;
  /** @returns 不包含凭据片段的 TMDB 配置与健康。 */
  getTmdbIntegration(): Promise<TmdbIntegration>;
  /** @param body - 仅用于此次调用的候选凭据。@returns 不持久化的连接测试结果。 */
  testTmdbConnection(body: TmdbConnectionTestRequest): Promise<TmdbConnectionTestResult>;
  /** @param body - 新配置。@param configVersion - `If-Match` 乐观并发版本。 */
  putTmdbIntegration(body: PutTmdbIntegrationRequest, configVersion: number): Promise<TmdbIntegration>;
  /** @param configVersion - `If-Match` 乐观并发版本。 */
  deleteTmdbIntegration(configVersion: number): Promise<void>;
  /** @param inboxDirectoryId - 收件目录 UUID。 */
  getDiscoveryPolicy(inboxDirectoryId: string): Promise<DiscoveryPolicy>;
  /** @param inboxDirectoryId - 收件目录 UUID。@param body - 新策略。@param configVersion - `If-Match` 版本。 */
  putDiscoveryPolicy(inboxDirectoryId: string, body: PutDiscoveryPolicyRequest, configVersion: number): Promise<DiscoveryPolicy>;
  /** @param options - 任务中心筛选；字符串仅保留为旧调用方的游标兼容形式。@returns 一页单文件任务。 */
  listProcessingTasks(options?: ProcessingTaskListOptions | string): Promise<ProcessingTaskPage>;
  /** @param id - ProcessingTask UUID。 */
  getProcessingTask(id: string): Promise<ProcessingTask>;
  /** @param id - ProcessingTask UUID。 */
  getProcessingTaskIdentification(id: string): Promise<IdentificationDetail>;
  /** @param id - ProcessingTask UUID。@param idempotencyKey - 安全重试键。 */
  retryProcessingTask(id: string, idempotencyKey: string): Promise<ProcessingTask>;
  /** @param id - ProcessingTask UUID。@param idempotencyKey - 安全取消键。 */
  cancelProcessingTask(id: string, idempotencyKey: string): Promise<ProcessingTask>;
  /** @param options - 可选游标和稳定筛选。@returns 一页活动审核案例。 */
  listReviewCases(options?: ReviewCaseListOptions): Promise<ReviewCasePage>;
  /** @param id - ReviewCase UUID。 */
  getReviewCase(id: string): Promise<ReviewCase>;
  /** @param id - ReviewCase UUID。@param options - 服务端候选查询。 */
  searchReviewCandidates(id: string, options: ReviewCandidateSearchOptions): Promise<ReviewCandidatePage>;
  /** @param id - ReviewCase UUID。@param version - 当前 case 版本。@param idempotencyKey - 安全重放键。@param body - 严格人工决定联合。 */
  submitReviewDecision(id: string, version: number, idempotencyKey: string, body: ReviewDecisionRequest): Promise<TaskDecisionReceipt>;
  /** @param options - 正式媒体筛选与游标。 */
  listMediaItems(options?: MediaItemListOptions): Promise<MediaItemPage>;
  /** @param id - 正式 MediaItem UUID。 */
  getMediaItem(id: string): Promise<MediaItemDetail>;
  /** @param cursor - 不透明下一页游标。@returns 一页不含凭据的下载器连接。 */
  listDownloaderConnections(cursor?: string): Promise<DownloaderConnectionPage>;
  /** @param body - 新连接及只用于本次写入的凭据。@returns 已持久化脱敏连接。 */
  createDownloaderConnection(body: DownloaderConnectionInput): Promise<DownloaderConnection>;
  /** @param id - 下载器连接 UUID。@returns 其最新脱敏投影。 */
  getDownloaderConnection(id: string): Promise<DownloaderConnection>;
  /** @param id - 下载器连接 UUID。@param body - 替换配置。@param configVersion - `If-Match` 版本。 */
  updateDownloaderConnection(id: string, body: DownloaderConnectionInput, configVersion: number): Promise<DownloaderConnection>;
  /** @param id - 下载器连接 UUID。@param configVersion - `If-Match` 版本；此操作不改变远端任务。 */
  deleteDownloaderConnection(id: string, configVersion: number): Promise<void>;
  /** @param body - 仅用于此次调用的候选连接和凭据。@returns 不落盘的能力与健康结果。 */
  testDownloaderConnection(body: DownloaderConnectionInput): Promise<DownloaderConnectionTestResult>;
  /** @param options - 连接、状态、关键字和游标筛选。@returns 一页 MediaFlow 自有下载任务。 */
  listDownloadTasks(options?: DownloadTaskListOptions): Promise<DownloadTaskPage>;
  /** @param body - 连接、加密源和显示名。@param idempotencyKey - 响应丢失时复用的安全重放键。 */
  createDownloadTask(body: CreateDownloadTaskRequest, idempotencyKey: string): Promise<DownloadTask>;
  /** @param id - 下载任务 UUID。@returns 不含源、tracker 或远端路径的最新投影。 */
  getDownloadTask(id: string): Promise<DownloadTask>;
  /** @param cursor - 不透明下一页游标。@returns 一页不含 secret/feed URL 的来源。 */
  listAutomationSources(cursor?: string): Promise<AutomationSourcePage>;
  /** @param body - 严格来源联合；敏感字段只用于当前请求。 */
  createAutomationSource(body: AutomationSourceInput): Promise<AutomationSourceCreateResult>;
  /** @param id - 自动来源 UUID。@returns 最新脱敏投影。 */
  getAutomationSource(id: string): Promise<AutomationSource>;
  /** @param id - 自动来源 UUID。@param body - 完整替换配置。@param version - 当前 `If-Match` 版本。 */
  updateAutomationSource(id: string, body: AutomationSourceInput, version: number): Promise<AutomationSource>;
  /** @param id - 已禁用且无活动事件的来源。@param version - 当前 `If-Match` 版本。 */
  deleteAutomationSource(id: string, version: number): Promise<void>;
  /** @param body - 仅用于本次调用的候选来源。@returns 不落盘健康结果。 */
  testAutomationSource(body: AutomationSourceInput): Promise<AutomationSourceConnectionTestResult>;
  /** @param id - Webhook 来源 UUID。@param version - 当前版本。@param idempotencyKey - 轮换意图重放键。 */
  rotateAutomationWebhookSecret(id: string, version: number, idempotencyKey: string): Promise<WebhookSecretReceipt>;
  /** @param options - 来源、状态、动作和游标筛选。 */
  listAutomationEvents(options?: AutomationEventListOptions): Promise<AutomationEventPage>;
  /** @param id - automation event UUID。 */
  getAutomationEvent(id: string): Promise<AutomationEvent>;
  /** @param id - 可恢复事件 UUID。@param idempotencyKey - 重试意图的稳定键。 */
  retryAutomationEvent(id: string, idempotencyKey: string): Promise<AutomationEvent>;
  /** @param id - 尚未关联下游的事件 UUID。@param idempotencyKey - 取消意图的稳定键。 */
  cancelAutomationEvent(id: string, idempotencyKey: string): Promise<AutomationEvent>;
  /** @returns 已脱敏的本地识别增强器配置。 */
  getIdentificationEnhancer(): Promise<IdentificationEnhancer>;
  /** @param body - 候选 Ollama 配置。@returns 不落盘的能力与健康。 */
  testIdentificationEnhancer(body: IdentificationEnhancerInput): Promise<IdentificationEnhancerConnectionTestResult>;
  /** @param body - 完整 Ollama 配置。@param version - 当前 `If-Match` 版本。 */
  putIdentificationEnhancer(body: IdentificationEnhancerInput, version: number): Promise<IdentificationEnhancer>;
  /** @param cursor - 不透明下一页游标。@returns 一页版本化整理目标。 */
  listOrganizationTargets(cursor?: string): Promise<OrganizationTargetPage>;
  /** @param body - 候选能力根和相对路径。@returns 不持久化也不创建目录的检查结果。 */
  preflightOrganizationTarget(body: OrganizationTargetPreflightRequest): Promise<OrganizationTargetPreflight>;
  /** @param body - 目标、固定 profile 与有界规则。@returns 已持久化目标投影。 */
  createOrganizationTarget(body: OrganizationTargetInput): Promise<OrganizationTarget>;
  /** @param id - 整理目标 UUID。@returns 当前版本投影。 */
  getOrganizationTarget(id: string): Promise<OrganizationTarget>;
  /** @param id - 整理目标 UUID。@param body - 完整替换聚合。@param version - 当前 `If-Match` 版本。 */
  updateOrganizationTarget(id: string, body: OrganizationTargetInput, version: number): Promise<OrganizationTarget>;
  /** @param id - 未被活动计划引用的目标 UUID。@param version - 当前 `If-Match` 版本；不会删除媒体文件。 */
  deleteOrganizationTarget(id: string, version: number): Promise<void>;
  /** @param id - ProcessingTask UUID。@returns 当前安全整理投影；未规划时返回显式空状态。 */
  getProcessingTaskOrganization(id: string): Promise<ProcessingTaskOrganization>;
  /** @param id - ProcessingTask UUID。@param idempotencyKey - 当前重算意图的稳定键。 */
  recalculateProcessingTaskOrganization(id: string, idempotencyKey: string): Promise<ProcessingTaskOrganization>;
  /** @param id - ProcessingTask UUID。@param planVersion - 当前不可变计划版本。@param idempotencyKey - 一次性执行意图的稳定键。 */
  executeProcessingTaskOrganization(id: string, planVersion: number, idempotencyKey: string): Promise<ProcessingTaskOrganization>;
  /** @param id - ProcessingTask UUID。@param resultVersion - 服务端判定可回滚的结果版本。@param idempotencyKey - 回滚意图的稳定键。 */
  rollbackProcessingTaskOrganization(id: string, resultVersion: number, idempotencyKey: string): Promise<ProcessingTaskOrganization>;
  /** @param token - 后续非 GET 请求发送的 CSRF 令牌；传入 `null` 会将其从内存清除。 */
  setCsrfToken(token: string | null): void;
}

const operations = {
  getBootstrapStatus: ["GET", "/api/v1/system/bootstrap-status"],
  bootstrapSystem: ["POST", "/api/v1/system/bootstrap"],
  createSession: ["POST", "/api/v1/sessions"],
  getSession: ["GET", "/api/v1/session"],
  deleteSession: ["DELETE", "/api/v1/session"],
  listDeploymentRoots: ["GET", "/api/v1/deployment-roots"],
  preflightInboxDirectory: ["POST", "/api/v1/inbox-directories/preflight"],
  listInboxDirectories: ["GET", "/api/v1/inbox-directories"],
  createInboxDirectory: ["POST", "/api/v1/inbox-directories"],
  getInboxDirectory: ["GET", "/api/v1/inbox-directories/{inboxDirectoryId}"],
  createScanTask: ["POST", "/api/v1/inbox-directories/{inboxDirectoryId}/scan-tasks"],
  listScanTasks: ["GET", "/api/v1/scan-tasks"],
  getScanTask: ["GET", "/api/v1/scan-tasks/{scanTaskId}"],
  retryScanTask: ["POST", "/api/v1/scan-tasks/{scanTaskId}/attempts"],
  cancelScanTask: ["POST", "/api/v1/scan-tasks/{scanTaskId}/cancel"],
  listScanTaskFiles: ["GET", "/api/v1/scan-tasks/{scanTaskId}/files"],
  listScanTaskErrors: ["GET", "/api/v1/scan-tasks/{scanTaskId}/errors"],
  getTmdbIntegration: ["GET", "/api/v1/integrations/tmdb"],
  testTmdbConnection: ["POST", "/api/v1/integrations/tmdb/connection-tests"],
  putTmdbIntegration: ["PUT", "/api/v1/integrations/tmdb"],
  deleteTmdbIntegration: ["DELETE", "/api/v1/integrations/tmdb"],
  getDiscoveryPolicy: ["GET", "/api/v1/inbox-directories/{inboxDirectoryId}/discovery-policy"],
  putDiscoveryPolicy: ["PUT", "/api/v1/inbox-directories/{inboxDirectoryId}/discovery-policy"],
  listProcessingTasks: ["GET", "/api/v1/processing-tasks"],
  getProcessingTask: ["GET", "/api/v1/processing-tasks/{processingTaskId}"],
  getProcessingTaskIdentification: ["GET", "/api/v1/processing-tasks/{processingTaskId}/identification"],
  retryProcessingTask: ["POST", "/api/v1/processing-tasks/{processingTaskId}/attempts"],
  cancelProcessingTask: ["POST", "/api/v1/processing-tasks/{processingTaskId}/cancel"],
  listReviewCases: ["GET", "/api/v1/review-cases"],
  getReviewCase: ["GET", "/api/v1/review-cases/{reviewCaseId}"],
  searchReviewCandidates: ["GET", "/api/v1/review-cases/{reviewCaseId}/candidates"],
  submitReviewDecision: ["POST", "/api/v1/review-cases/{reviewCaseId}/decisions"],
  listMediaItems: ["GET", "/api/v1/media-items"],
  getMediaItem: ["GET", "/api/v1/media-items/{mediaItemId}"],
  listDownloaderConnections: ["GET", "/api/v1/downloader-connections"],
  createDownloaderConnection: ["POST", "/api/v1/downloader-connections"],
  getDownloaderConnection: ["GET", "/api/v1/downloader-connections/{downloaderConnectionId}"],
  updateDownloaderConnection: ["PUT", "/api/v1/downloader-connections/{downloaderConnectionId}"],
  deleteDownloaderConnection: ["DELETE", "/api/v1/downloader-connections/{downloaderConnectionId}"],
  testDownloaderConnection: ["POST", "/api/v1/downloader-connections/connection-tests"],
  listDownloadTasks: ["GET", "/api/v1/download-tasks"],
  createDownloadTask: ["POST", "/api/v1/download-tasks"],
  getDownloadTask: ["GET", "/api/v1/download-tasks/{downloadTaskId}"],
  listAutomationSources: ["GET", "/api/v1/automation-sources"],
  createAutomationSource: ["POST", "/api/v1/automation-sources"],
  getAutomationSource: ["GET", "/api/v1/automation-sources/{automationSourceId}"],
  updateAutomationSource: ["PUT", "/api/v1/automation-sources/{automationSourceId}"],
  deleteAutomationSource: ["DELETE", "/api/v1/automation-sources/{automationSourceId}"],
  testAutomationSource: ["POST", "/api/v1/automation-sources/connection-tests"],
  rotateAutomationWebhookSecret: ["POST", "/api/v1/automation-sources/{automationSourceId}/secret-rotations"],
  listAutomationEvents: ["GET", "/api/v1/automation-events"],
  getAutomationEvent: ["GET", "/api/v1/automation-events/{automationEventId}"],
  retryAutomationEvent: ["POST", "/api/v1/automation-events/{automationEventId}/retries"],
  cancelAutomationEvent: ["POST", "/api/v1/automation-events/{automationEventId}/cancellations"],
  getIdentificationEnhancer: ["GET", "/api/v1/identification-enhancer"],
  testIdentificationEnhancer: ["POST", "/api/v1/identification-enhancer/connection-tests"],
  putIdentificationEnhancer: ["PUT", "/api/v1/identification-enhancer"],
  listOrganizationTargets: ["GET", "/api/v1/organization-targets"],
  preflightOrganizationTarget: ["POST", "/api/v1/organization-targets/preflights"],
  createOrganizationTarget: ["POST", "/api/v1/organization-targets"],
  getOrganizationTarget: ["GET", "/api/v1/organization-targets/{organizationTargetId}"],
  updateOrganizationTarget: ["PUT", "/api/v1/organization-targets/{organizationTargetId}"],
  deleteOrganizationTarget: ["DELETE", "/api/v1/organization-targets/{organizationTargetId}"],
  getProcessingTaskOrganization: ["GET", "/api/v1/processing-tasks/{processingTaskId}/organization"],
  recalculateProcessingTaskOrganization: ["POST", "/api/v1/processing-tasks/{processingTaskId}/organization/recalculations"],
  executeProcessingTaskOrganization: ["POST", "/api/v1/processing-tasks/{processingTaskId}/organization/executions"],
  rollbackProcessingTaskOrganization: ["POST", "/api/v1/processing-tasks/{processingTaskId}/organization/rollbacks"],
} as const;

type OperationId = keyof typeof operations;

const errorCodes = new Set([
  "bootstrap.already_completed", "bootstrap.invalid_secret", "auth.invalid_credentials", "auth.rate_limited", "session.expired", "csrf.invalid", "origin.untrusted", "root.not_found", "root.unavailable", "path.invalid", "path.escape", "path.symlink_forbidden", "inbox.overlap", "inbox.not_found", "task.not_found", "task.invalid_state", "task.lease_lost", "integration.not_configured", "integration.unauthorized", "integration.rate_limited", "integration.unsupported-version", "provider.unavailable", "provider.timeout", "provider.response-too-large", "provider.invalid-response", "download.correlation-ambiguous", "download.remote-missing", "automation.source-disabled", "automation.signature-invalid", "automation.replay", "automation.action-invalid", "automation.payload-invalid", "automation.downstream-conflict", "organization.root-read-only", "organization.target-overlap", "organization.target-unavailable", "organization.path-outside-root", "organization.target-exists", "organization.source-changed", "organization.hardlink-cross-device", "organization.operation-unsupported", "organization.journal-ambiguous", "revision.changed", "event.cursor_expired", "request.conflict", "validation.failed", "internal.error",
]);
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;
const plainObject = (value: unknown): value is Record<string, unknown> => Boolean(value && typeof value === "object" && !Array.isArray(value) && Object.getPrototypeOf(value) === Object.prototype);
const safeDetails = (value: unknown): boolean => value === undefined || (plainObject(value) && Object.keys(value).every((key) => !["__proto__", "constructor", "prototype"].includes(key)) && Object.values(value).every((item) => item === null || ["string", "number", "boolean"].includes(typeof item)));

function isApiErrorBody(value: unknown): value is ApiErrorBody {
  if (!plainObject(value) || Object.keys(value).length !== 1 || !Object.hasOwn(value, "error") || !plainObject(value.error)) return false;
  const error = value.error;
  const allowed = ["code", "message", "request_id", "details"];
  return Object.keys(error).every((key) => allowed.includes(key)) && typeof error.code === "string" && errorCodes.has(error.code) && typeof error.message === "string" && uuid.test(error.request_id as string) && safeDetails(error.details);
}

/**
 * 创建有状态、基于 fetch 的 MediaFlow v1 传输客户端。
 *
 * @param options - 可选的绝对基准 URL 和 fetch 实现。未指定基准 URL 时，请求保持同源相对 URL；未覆盖 fetch
 * 时使用当前全局实现。
 * @returns 客户端方法会设置请求 ID、编码路径和查询值、发送同源凭据，并向非 GET 操作附加最新的内存 CSRF 令牌。
 * @throws {MediaFlowApiError} 响应不成功时抛出；仅在验证通过后保留错误响应体。
 * @throws {TypeError} 构造 URL 失败时抛出；提供的 fetch 实现所抛错误会原样透传。
 * @remarks `setCsrfToken` 仅修改闭包本地内存；客户端不会持久化凭据或令牌。
 */
export function createMediaFlowClient(options: { baseUrl?: string; fetch?: typeof globalThis.fetch } = {}): MediaFlowClient {
  const fetcher = options.fetch ?? globalThis.fetch;
  let csrfToken: string | null = null;

  async function request<T>(operation: OperationId, requestOptions: { path?: Record<string, string>; body?: unknown; cursor?: string; query?: Record<string, string | undefined>; idempotencyKey?: string; ifMatch?: number } = {}): Promise<T> {
    const [method, template] = operations[operation];
    const path = Object.entries(requestOptions.path ?? {}).reduce<string>((value, [key, id]) => value.replace(`{${key}}`, encodeURIComponent(id)), template);
    const url = new URL(path, options.baseUrl ?? "http://mediaflow.local");
    if (requestOptions.cursor) url.searchParams.set("cursor", requestOptions.cursor);
    for (const [key, value] of Object.entries(requestOptions.query ?? {})) {
      if (value !== undefined) url.searchParams.set(key, value);
    }
    const headers: Record<string, string> = { "X-Request-ID": crypto.randomUUID() };
    if (requestOptions.body !== undefined) headers["Content-Type"] = "application/json";
    if (requestOptions.idempotencyKey) headers["Idempotency-Key"] = requestOptions.idempotencyKey;
    if (requestOptions.ifMatch !== undefined) headers["If-Match"] = String(requestOptions.ifMatch);
    if (method !== "GET" && csrfToken) headers["X-CSRF-Token"] = csrfToken;
    const response = await fetcher(options.baseUrl ? url.toString() : `${path}${url.search}`, { method, headers, body: requestOptions.body === undefined ? undefined : JSON.stringify(requestOptions.body), credentials: "same-origin" });
    if (!response.ok) {
      let body: ApiErrorBody | undefined;
      if (response.headers.get("content-type")?.includes("application/json")) {
        try {
          const parsed: unknown = await response.json();
          if (isApiErrorBody(parsed)) body = parsed;
        } catch {
          body = undefined;
        }
      }
      throw new MediaFlowApiError(response.status, body);
    }
    return response.status === 204 ? undefined as T : await response.json() as T;
  }

  return {
    getBootstrapStatus: () => request("getBootstrapStatus"),
    bootstrap: (body) => request("bootstrapSystem", { body }),
    createSession: (body) => request("createSession", { body }),
    getSession: () => request("getSession"),
    deleteSession: () => request("deleteSession"),
    listDeploymentRoots: () => request("listDeploymentRoots"),
    preflightInboxDirectory: (body) => request("preflightInboxDirectory", { body }),
    listInboxDirectories: (cursor) => request("listInboxDirectories", { cursor }),
    createInboxDirectory: (body) => request("createInboxDirectory", { body }),
    getInboxDirectory: (id) => request("getInboxDirectory", { path: { inboxDirectoryId: id } }),
    createScanTask: (inboxDirectoryId, idempotencyKey) => request("createScanTask", { path: { inboxDirectoryId }, idempotencyKey }),
    listScanTasks: (cursor) => request("listScanTasks", { cursor }),
    getScanTask: (id) => request("getScanTask", { path: { scanTaskId: id } }),
    retryScanTask: (id, idempotencyKey) => request("retryScanTask", { path: { scanTaskId: id }, idempotencyKey }),
    cancelScanTask: (id, idempotencyKey) => request("cancelScanTask", { path: { scanTaskId: id }, idempotencyKey }),
    listScanTaskFiles: (id, cursor) => request("listScanTaskFiles", { path: { scanTaskId: id }, cursor }),
    listScanTaskErrors: (id, cursor) => request("listScanTaskErrors", { path: { scanTaskId: id }, cursor }),
    getTmdbIntegration: () => request("getTmdbIntegration"),
    testTmdbConnection: (body) => request("testTmdbConnection", { body }),
    putTmdbIntegration: (body, configVersion) => request("putTmdbIntegration", { body, ifMatch: configVersion }),
    deleteTmdbIntegration: (configVersion) => request("deleteTmdbIntegration", { ifMatch: configVersion }),
    getDiscoveryPolicy: (inboxDirectoryId) => request("getDiscoveryPolicy", { path: { inboxDirectoryId } }),
    putDiscoveryPolicy: (inboxDirectoryId, body, configVersion) => request("putDiscoveryPolicy", { path: { inboxDirectoryId }, body, ifMatch: configVersion }),
    listProcessingTasks: (options = {}) => {
      const normalized = typeof options === "string" ? { cursor: options } : options;
      return request("listProcessingTasks", {
        cursor: normalized.cursor,
        query: {
          view: normalized.view,
          stage: normalized.stage,
          status: normalized.status,
          inbox_directory_id: normalized.inboxDirectoryId,
          q: normalized.query,
        },
      });
    },
    getProcessingTask: (id) => request("getProcessingTask", { path: { processingTaskId: id } }),
    getProcessingTaskIdentification: (id) => request("getProcessingTaskIdentification", { path: { processingTaskId: id } }),
    retryProcessingTask: (id, idempotencyKey) => request("retryProcessingTask", { path: { processingTaskId: id }, idempotencyKey }),
    cancelProcessingTask: (id, idempotencyKey) => request("cancelProcessingTask", { path: { processingTaskId: id }, idempotencyKey }),
    listReviewCases: (options = {}) => request("listReviewCases", {
      cursor: options.cursor,
      query: { decision_level: options.level, inbox_directory_id: options.inboxDirectoryId, updated_before: options.updatedBefore },
    }),
    getReviewCase: (id) => request("getReviewCase", { path: { reviewCaseId: id } }),
    searchReviewCandidates: (id, options) => request("searchReviewCandidates", {
      path: { reviewCaseId: id },
      query: {
        q: options.query,
        media_type: options.mediaType,
        locale: options.locale,
        limit: options.limit === undefined ? undefined : String(options.limit),
      },
    }),
    submitReviewDecision: (id, version, idempotencyKey, body) => request("submitReviewDecision", {
      path: { reviewCaseId: id }, body, idempotencyKey, ifMatch: version,
    }),
    listMediaItems: (options = {}) => request("listMediaItems", {
      cursor: options.cursor,
      query: { type: options.type, library_id: options.libraryId, local_status: options.localStatus, q: options.query },
    }),
    getMediaItem: (id) => request("getMediaItem", { path: { mediaItemId: id } }),
    listDownloaderConnections: (cursor) => request("listDownloaderConnections", { cursor }),
    createDownloaderConnection: (body) => request("createDownloaderConnection", { body }),
    getDownloaderConnection: (id) => request("getDownloaderConnection", { path: { downloaderConnectionId: id } }),
    updateDownloaderConnection: (id, body, configVersion) => request("updateDownloaderConnection", {
      path: { downloaderConnectionId: id }, body, ifMatch: configVersion,
    }),
    deleteDownloaderConnection: (id, configVersion) => request("deleteDownloaderConnection", {
      path: { downloaderConnectionId: id }, ifMatch: configVersion,
    }),
    testDownloaderConnection: (body) => request("testDownloaderConnection", { body }),
    listDownloadTasks: (options = {}) => request("listDownloadTasks", {
      cursor: options.cursor,
      query: { connection_id: options.connectionId, status: options.status, q: options.query },
    }),
    createDownloadTask: (body, idempotencyKey) => request("createDownloadTask", { body, idempotencyKey }),
    getDownloadTask: (id) => request("getDownloadTask", { path: { downloadTaskId: id } }),
    listAutomationSources: (cursor) => request("listAutomationSources", { cursor }),
    createAutomationSource: (body) => request("createAutomationSource", { body }),
    getAutomationSource: (id) => request("getAutomationSource", { path: { automationSourceId: id } }),
    updateAutomationSource: (id, body, version) => request("updateAutomationSource", {
      path: { automationSourceId: id }, body, ifMatch: version,
    }),
    deleteAutomationSource: (id, version) => request("deleteAutomationSource", {
      path: { automationSourceId: id }, ifMatch: version,
    }),
    testAutomationSource: (body) => request("testAutomationSource", { body }),
    rotateAutomationWebhookSecret: (id, version, idempotencyKey) => request("rotateAutomationWebhookSecret", {
      path: { automationSourceId: id }, ifMatch: version, idempotencyKey,
    }),
    listAutomationEvents: (options = {}) => request("listAutomationEvents", {
      cursor: options.cursor,
      query: { source_id: options.sourceId, status: options.status, action: options.action },
    }),
    getAutomationEvent: (id) => request("getAutomationEvent", { path: { automationEventId: id } }),
    retryAutomationEvent: (id, idempotencyKey) => request("retryAutomationEvent", {
      path: { automationEventId: id }, idempotencyKey,
    }),
    cancelAutomationEvent: (id, idempotencyKey) => request("cancelAutomationEvent", {
      path: { automationEventId: id }, idempotencyKey,
    }),
    getIdentificationEnhancer: () => request("getIdentificationEnhancer"),
    testIdentificationEnhancer: (body) => request("testIdentificationEnhancer", { body }),
    putIdentificationEnhancer: (body, version) => request("putIdentificationEnhancer", { body, ifMatch: version }),
    listOrganizationTargets: (cursor) => request("listOrganizationTargets", { cursor }),
    preflightOrganizationTarget: (body) => request("preflightOrganizationTarget", { body }),
    createOrganizationTarget: (body) => request("createOrganizationTarget", { body }),
    getOrganizationTarget: (id) => request("getOrganizationTarget", { path: { organizationTargetId: id } }),
    updateOrganizationTarget: (id, body, version) => request("updateOrganizationTarget", {
      path: { organizationTargetId: id }, body, ifMatch: version,
    }),
    deleteOrganizationTarget: (id, version) => request("deleteOrganizationTarget", {
      path: { organizationTargetId: id }, ifMatch: version,
    }),
    getProcessingTaskOrganization: (id) => request("getProcessingTaskOrganization", { path: { processingTaskId: id } }),
    recalculateProcessingTaskOrganization: (id, idempotencyKey) => request("recalculateProcessingTaskOrganization", {
      path: { processingTaskId: id }, idempotencyKey,
    }),
    executeProcessingTaskOrganization: (id, planVersion, idempotencyKey) => request("executeProcessingTaskOrganization", {
      path: { processingTaskId: id }, body: { plan_version: planVersion }, idempotencyKey,
    }),
    rollbackProcessingTaskOrganization: (id, resultVersion, idempotencyKey) => request("rollbackProcessingTaskOrganization", {
      path: { processingTaskId: id }, body: { result_version: resultVersion }, idempotencyKey,
    }),
    setCsrfToken: (token) => { csrfToken = token; },
  };
}
