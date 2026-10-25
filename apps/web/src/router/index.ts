import type { MediaFlowClient } from "@mediaflow/api-client-ts";
import type { Pinia } from "pinia";
import {
  createMemoryHistory,
  createRouter,
  createWebHistory,
  type RouteRecordRaw,
  type Router,
} from "vue-router";
import AppShell from "../app/AppShell.vue";
import { registerBootstrapInvalidator } from "../app/bootstrap";
import { useConnectivityStore } from "../app/connectivity";
import { useSessionStore } from "../app/session";
// 首进关键路径（初始化探测 → 设置/登录）保持同步导入；认证后业务视图懒加载为独立 chunk，
// 路由名、守卫、重定向与查询语义不变。chunk 加载失败由 createAppRouter 的 onError 恢复。
import LoginView from "../views/LoginView.vue";
import SetupView from "../views/SetupView.vue";

/**
 * MediaFlow 路由守卫所需的运行时服务。
 *
 * 两个客户端方法都可能拒绝；守卫会将这些失败转换为登录重定向或连通性状态。
 */
export interface RouterDependencies {
  pinia: Pinia;
  getBootstrapStatus: MediaFlowClient["getBootstrapStatus"];
  getSession: MediaFlowClient["getSession"];
}

const routes: RouteRecordRaw[] = [
  { path: "/setup", name: "setup", component: SetupView },
  { path: "/login", name: "login", component: LoginView },
  {
    path: "/",
    component: AppShell,
    children: [
      { path: "inbox-directories", name: "inbox-directories", component: () => import("../views/InboxListView.vue"), meta: { requiresAuth: true } },
      { path: "inbox-directories/:id", name: "inbox-directory", component: () => import("../views/InboxDetailView.vue"), meta: { requiresAuth: true } },
      { path: "tasks", name: "tasks", component: () => import("../views/TaskCenterView.vue"), meta: { requiresAuth: true } },
      { path: "tasks/:id", name: "task", component: () => import("../views/ProcessingTaskDetailView.vue"), meta: { requiresAuth: true } },
      { path: "review-cases/:id", name: "review-case", component: () => import("../views/ReviewCaseView.vue"), meta: { requiresAuth: true } },
      { path: "media", name: "media", component: () => import("../views/MediaListView.vue"), meta: { requiresAuth: true } },
      { path: "media/:id", name: "media-item", component: () => import("../views/MediaDetailView.vue"), meta: { requiresAuth: true } },
      { path: "connections/downloaders", name: "downloader-connections", component: () => import("../views/DownloaderConnectionsView.vue"), meta: { requiresAuth: true } },
      { path: "connections/downloaders/:id", name: "downloader-connection", component: () => import("../views/DownloaderConnectionDetailView.vue"), meta: { requiresAuth: true } },
      { path: "downloads", name: "download-tasks", component: () => import("../views/DownloadTaskListView.vue"), meta: { requiresAuth: true } },
      { path: "downloads/:id", name: "download-task", component: () => import("../views/DownloadTaskDetailView.vue"), meta: { requiresAuth: true } },
      { path: "automation/sources", name: "automation-sources", component: () => import("../views/AutomationSourcesView.vue"), meta: { requiresAuth: true } },
    { path: "capability-events", name: "capability-events", component: () => import("../views/CapabilityEventsView.vue"), meta: { requiresAuth: true } },
    { path: "notifications", name: "notifications", component: () => import("../views/NotificationsView.vue"), meta: { requiresAuth: true } },
      { path: "automation/sources/:id", name: "automation-source", component: () => import("../views/AutomationSourceDetailView.vue"), meta: { requiresAuth: true } },
      { path: "automation/events/:id", name: "automation-event", component: () => import("../views/AutomationEventDetailView.vue"), meta: { requiresAuth: true } },
      { path: "organization/targets", name: "organization-targets", component: () => import("../views/OrganizationTargetsView.vue"), meta: { requiresAuth: true } },
      { path: "organization/targets/:id", name: "organization-target", component: () => import("../views/OrganizationTargetDetailView.vue"), meta: { requiresAuth: true } },
      { path: "tasks/:id/files", redirect: (to) => ({ name: "scan-task-files", params: to.params, query: to.query }) },
      { path: "scan-tasks", name: "scan-tasks", component: () => import("../views/ScanTaskListView.vue"), meta: { requiresAuth: true } },
      { path: "scan-tasks/:id", name: "scan-task", component: () => import("../views/ScanTaskDetailView.vue"), meta: { requiresAuth: true } },
      { path: "scan-tasks/:id/files", name: "scan-task-files", component: () => import("../views/RawFilesView.vue"), meta: { requiresAuth: true } },
    ],
  },
];

function isUnauthorized(error: unknown): boolean {
  return typeof error === "object" && error !== null && "status" in error && error.status === 401;
}

/** 懒加载 chunk 拉取失败（如部署更新后旧 chunk 404）的特征：Vite 动态 import 的原生报错形态。 */
function isChunkLoadError(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  const message = (error as { message?: unknown }).message;
  const text = typeof message === "string" ? message : "";
  return (error as { type?: unknown }).type === "module" || /dynamically imported module|error loading dynamically imported/i.test(text);
}

/** chunk 失效的一次性整页重载标记；同一目标只自动重载一次，避免循环。 */
const CHUNK_RELOAD_FLAG = "mediaflow-chunk-reload-target";

/**
 * 创建应用路由器并安装初始化/会话导航守卫。
 *
 * @param dependencies - 守卫使用的 Pinia 实例和客户端探测函数。
 * @returns 浏览器外返回内存历史路由器，浏览器内返回 Web 历史路由器。
 * @remarks 导航探测会修改连通性和会话 store。初始化状态会被缓存，直至设置操作使其失效；未认证的业务路由会保存
 * 已验证的返回位置并重定向到登录页。探测失败会取消当前导航，而不是暴露未验证的路由。
 */
export function createAppRouter(dependencies: RouterDependencies): Router {
  const router = createRouter({
    history: typeof window === "undefined" ? createMemoryHistory() : createWebHistory(),
    routes,
  });
  let initializationRequired: boolean | null = null;
  const session = useSessionStore(dependencies.pinia);
  const connectivity = useConnectivityStore(dependencies.pinia);
  registerBootstrapInvalidator(router, () => { initializationRequired = null; });

  // 懒加载 chunk 拉取失败（典型：部署更新后旧入口引用的 chunk 已不存在）：同一目标只做一次
  // 整页重载以获取新入口与 chunk；二次失败不再重载，保留当前已加载视图由用户手动处理。
  router.onError((error, to) => {
    if (!isChunkLoadError(error) || typeof window === "undefined") return;
    let previous: string | null = null;
    try {
      previous = window.sessionStorage.getItem(CHUNK_RELOAD_FLAG);
      window.sessionStorage.setItem(CHUNK_RELOAD_FLAG, to.fullPath);
    } catch {
      // 存储不可用（隐私模式等）：仍执行一次重载。
    }
    if (previous === to.fullPath) return;
    window.location.assign(to.fullPath);
  });

  router.beforeEach(async (to, from) => {
    if (to.name === "task" && to.query.scan_context === "scan-task") {
      const query = { ...to.query };
      delete query.scan_context;
      return { name: "scan-task", params: to.params, query, replace: true };
    }
    if (initializationRequired === true && from.name === "setup" && to.name === "login") {
      initializationRequired = null;
    }
    if (initializationRequired === null) {
      connectivity.begin(to.fullPath);
      try {
        const status = await dependencies.getBootstrapStatus();
        initializationRequired = status.requires_initialization;
        connectivity.connected();
      } catch (error) {
        connectivity.failed(error, to.fullPath);
        return false;
      }
    }

    if (initializationRequired) {
      return to.name === "setup" ? true : { name: "setup", replace: true };
    }
    if (to.name === "setup") return { name: "login", replace: true };
    if (to.name === "login") return true;
    if (to.path === "/") return { name: "inbox-directories", replace: true };

    if (to.meta.requiresAuth) {
      if (session.signedOut) return { name: "login", replace: true };
      try {
        session.establish(await dependencies.getSession());
        connectivity.connected();
      } catch (error) {
        if (isUnauthorized(error)) {
          session.saveReturnLocation(to.fullPath);
          session.resetSensitiveState();
          return { name: "login", replace: true };
        }
        initializationRequired = null;
        connectivity.failed(error, to.fullPath);
        return false;
      }
    }
    return true;
  });

  return router;
}
