import type { MediaFlowClient } from "@mediaflow/api-client-ts";
import { inject } from "vue";
import { identityClientKey } from "../app/client";

function unavailable(): Promise<never> { return new Promise<never>(() => undefined); }

/**
 * 读取当前 Vue 应用提供的 MediaFlow 客户端。
 *
 * @returns 注入的客户端；若组件未处于应用提供者内渲染，则返回请求 Promise 保持等待且不会抛错的占位对象。
 * @remarks 回退对象可防止隔离渲染时意外访问网络；只有其 CSRF 设置器是空操作。
 */
export function useMediaFlowClient(): MediaFlowClient {
  const injected = inject(identityClientKey, null);
  if (injected) return injected;
  return {
    getBootstrapStatus: unavailable, bootstrap: unavailable, createSession: unavailable, getSession: unavailable, deleteSession: unavailable,
    listDeploymentRoots: unavailable, preflightInboxDirectory: unavailable, listInboxDirectories: unavailable,
    createInboxDirectory: unavailable, getInboxDirectory: unavailable, createScanTask: unavailable, listScanTasks: unavailable,
    getScanTask: unavailable, retryScanTask: unavailable, cancelScanTask: unavailable, listScanTaskFiles: unavailable,
    listScanTaskErrors: unavailable, getTmdbIntegration: unavailable, testTmdbConnection: unavailable,
    putTmdbIntegration: unavailable, deleteTmdbIntegration: unavailable, getDiscoveryPolicy: unavailable,
    putDiscoveryPolicy: unavailable, listProcessingTasks: unavailable, getProcessingTask: unavailable,
    getProcessingTaskIdentification: unavailable, retryProcessingTask: unavailable, cancelProcessingTask: unavailable,
    listOrganizationTargets: unavailable, preflightOrganizationTarget: unavailable, createOrganizationTarget: unavailable,
    getOrganizationTarget: unavailable, updateOrganizationTarget: unavailable, deleteOrganizationTarget: unavailable,
    getProcessingTaskOrganization: unavailable, recalculateProcessingTaskOrganization: unavailable,
    executeProcessingTaskOrganization: unavailable, rollbackProcessingTaskOrganization: unavailable,
    listReviewCases: unavailable, getReviewCase: unavailable, searchReviewCandidates: unavailable,
    submitReviewDecision: unavailable, listMediaItems: unavailable, getMediaItem: unavailable,
    listDownloaderConnections: unavailable, createDownloaderConnection: unavailable,
    getDownloaderConnection: unavailable, updateDownloaderConnection: unavailable,
    deleteDownloaderConnection: unavailable, testDownloaderConnection: unavailable,
    listDownloadTasks: unavailable, createDownloadTask: unavailable, getDownloadTask: unavailable,
    listAutomationSources: unavailable, createAutomationSource: unavailable, getAutomationSource: unavailable,
    updateAutomationSource: unavailable, deleteAutomationSource: unavailable, testAutomationSource: unavailable,
    rotateAutomationWebhookSecret: unavailable, listAutomationEvents: unavailable, getAutomationEvent: unavailable,
    retryAutomationEvent: unavailable, cancelAutomationEvent: unavailable, getIdentificationEnhancer: unavailable,
    testIdentificationEnhancer: unavailable, putIdentificationEnhancer: unavailable,
    setCsrfToken: () => undefined,
  };
}
