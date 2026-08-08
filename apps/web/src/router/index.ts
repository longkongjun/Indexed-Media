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
import LoginView from "../views/LoginView.vue";
import SetupView from "../views/SetupView.vue";
import InboxListView from "../views/InboxListView.vue";
import InboxDetailView from "../views/InboxDetailView.vue";
import ScanTaskListView from "../views/ScanTaskListView.vue";
import ScanTaskDetailView from "../views/ScanTaskDetailView.vue";
import RawFilesView from "../views/RawFilesView.vue";
import TaskCenterView from "../views/TaskCenterView.vue";
import ProcessingTaskDetailView from "../views/ProcessingTaskDetailView.vue";
import ReviewCaseView from "../views/ReviewCaseView.vue";
import MediaListView from "../views/MediaListView.vue";
import MediaDetailView from "../views/MediaDetailView.vue";
import DownloaderConnectionsView from "../views/DownloaderConnectionsView.vue";
import DownloaderConnectionDetailView from "../views/DownloaderConnectionDetailView.vue";
import DownloadTaskListView from "../views/DownloadTaskListView.vue";
import DownloadTaskDetailView from "../views/DownloadTaskDetailView.vue";
import OrganizationTargetsView from "../views/OrganizationTargetsView.vue";
import OrganizationTargetDetailView from "../views/OrganizationTargetDetailView.vue";
import AutomationSourcesView from "../views/AutomationSourcesView.vue";
import AutomationSourceDetailView from "../views/AutomationSourceDetailView.vue";
import AutomationEventDetailView from "../views/AutomationEventDetailView.vue";

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
      { path: "inbox-directories", name: "inbox-directories", component: InboxListView, meta: { requiresAuth: true } },
      { path: "inbox-directories/:id", name: "inbox-directory", component: InboxDetailView, meta: { requiresAuth: true } },
      { path: "tasks", name: "tasks", component: TaskCenterView, meta: { requiresAuth: true } },
      { path: "tasks/:id", name: "task", component: ProcessingTaskDetailView, meta: { requiresAuth: true } },
      { path: "review-cases/:id", name: "review-case", component: ReviewCaseView, meta: { requiresAuth: true } },
      { path: "media", name: "media", component: MediaListView, meta: { requiresAuth: true } },
      { path: "media/:id", name: "media-item", component: MediaDetailView, meta: { requiresAuth: true } },
      { path: "connections/downloaders", name: "downloader-connections", component: DownloaderConnectionsView, meta: { requiresAuth: true } },
      { path: "connections/downloaders/:id", name: "downloader-connection", component: DownloaderConnectionDetailView, meta: { requiresAuth: true } },
      { path: "downloads", name: "download-tasks", component: DownloadTaskListView, meta: { requiresAuth: true } },
      { path: "downloads/:id", name: "download-task", component: DownloadTaskDetailView, meta: { requiresAuth: true } },
      { path: "automation/sources", name: "automation-sources", component: AutomationSourcesView, meta: { requiresAuth: true } },
      { path: "automation/sources/:id", name: "automation-source", component: AutomationSourceDetailView, meta: { requiresAuth: true } },
      { path: "automation/events/:id", name: "automation-event", component: AutomationEventDetailView, meta: { requiresAuth: true } },
      { path: "organization/targets", name: "organization-targets", component: OrganizationTargetsView, meta: { requiresAuth: true } },
      { path: "organization/targets/:id", name: "organization-target", component: OrganizationTargetDetailView, meta: { requiresAuth: true } },
      { path: "tasks/:id/files", redirect: (to) => ({ name: "scan-task-files", params: to.params, query: to.query }) },
      { path: "scan-tasks", name: "scan-tasks", component: ScanTaskListView, meta: { requiresAuth: true } },
      { path: "scan-tasks/:id", name: "scan-task", component: ScanTaskDetailView, meta: { requiresAuth: true } },
      { path: "scan-tasks/:id/files", name: "scan-task-files", component: RawFilesView, meta: { requiresAuth: true } },
    ],
  },
];

function isUnauthorized(error: unknown): boolean {
  return typeof error === "object" && error !== null && "status" in error && error.status === 401;
}

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
