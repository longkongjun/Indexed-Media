import { describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { createMediaFlowClient as createPublicClient } from "@mediaflow/api-client-ts";
import { MediaFlowApiError, createMediaFlowClient } from "../src/client.js";

describe("MediaFlowClient", () => {
  it("adds same-origin credentials, request id and csrf for mutations", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(
        new Response(JSON.stringify({ requires_initialization: false, version: "v1" }), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
      )
      .mockResolvedValueOnce(new Response(null, { status: 204 }));
    const client = createMediaFlowClient({ fetch });
    client.setCsrfToken("csrf-value");

    await client.getBootstrapStatus();
    await client.deleteSession();

    expect(fetch).toHaveBeenNthCalledWith(
      1,
      "/api/v1/system/bootstrap-status",
      expect.objectContaining({ credentials: "same-origin", method: "GET" }),
    );
    expect(fetch).toHaveBeenNthCalledWith(
      2,
      "/api/v1/session",
      expect.objectContaining({
        credentials: "same-origin",
        method: "DELETE",
        headers: expect.objectContaining({
          "X-CSRF-Token": "csrf-value",
          "X-Request-ID": expect.any(String),
        }),
      }),
    );
  });

  it("maps every operation, cursors, and idempotency keys to the contract paths", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    const inboxId = "019f0000-0000-7000-8000-000000000002";
    const taskId = "019f0000-0000-7000-8000-000000000003";
    await client.getBootstrapStatus();
    await client.bootstrap({ bootstrap_secret: "secret", administrator_name: "admin", password: "correct-horse-battery-staple" });
    await client.createSession({ administrator_name: "admin", password: "password" });
    await client.getSession();
    await client.listDeploymentRoots();
    if (false) {
      // @ts-expect-error 部署根目录是有界且不分页的配置列表
      await client.listDeploymentRoots("roots-cursor");
    }
    await client.preflightInboxDirectory({ root_id: "incoming", relative_path: "movies" });
    await client.listInboxDirectories("inbox-cursor");
    await client.createInboxDirectory({ root_id: "incoming", relative_path: "movies" });
    await client.getInboxDirectory(inboxId);
    await client.createScanTask(inboxId, "create-key");
    await client.listScanTasks("task-cursor");
    await client.getScanTask(taskId);
    await client.retryScanTask(taskId, "retry-key");
    await client.cancelScanTask(taskId, "cancel-key");
    await client.listScanTaskFiles(taskId, "files-cursor");
    await client.listScanTaskErrors(taskId, "errors-cursor");
    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/system/bootstrap-status", "/api/v1/system/bootstrap", "/api/v1/sessions", "/api/v1/session",
      "/api/v1/deployment-roots", "/api/v1/inbox-directories/preflight", "/api/v1/inbox-directories?cursor=inbox-cursor", "/api/v1/inbox-directories",
      `/api/v1/inbox-directories/${inboxId}`, `/api/v1/inbox-directories/${inboxId}/scan-tasks`, "/api/v1/scan-tasks?cursor=task-cursor", `/api/v1/scan-tasks/${taskId}`,
      `/api/v1/scan-tasks/${taskId}/attempts`, `/api/v1/scan-tasks/${taskId}/cancel`, `/api/v1/scan-tasks/${taskId}/files?cursor=files-cursor`, `/api/v1/scan-tasks/${taskId}/errors?cursor=errors-cursor`,
    ]);
    for (const [call, key] of [[9, "create-key"], [12, "retry-key"], [13, "cancel-key"]] as const) expect((fetch.mock.calls[call][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": key }));
  });

  it("maps M3 integration, discovery, processing, and review operations", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    const inboxId = "019f0000-0000-7000-8000-000000000002";
    const taskId = "019f0000-0000-7000-8000-000000000043";
    const reviewCaseId = "019f0000-0000-7000-8000-000000000048";
    const credentials = { api_read_access_token: "candidate-token-never-saved", locale: "zh-CN", region: "CN" } as const;

    await client.getTmdbIntegration();
    await client.testTmdbConnection(credentials);
    await client.putTmdbIntegration(credentials, 3);
    await client.deleteTmdbIntegration(4);
    await client.getDiscoveryPolicy(inboxId);
    await client.putDiscoveryPolicy(inboxId, { minimum_age_seconds: 60, stable_observation_interval_seconds: 30, reconcile_interval_seconds: 900, watcher_enabled: true }, 2);
    await client.listProcessingTasks("processing-cursor");
    await client.getProcessingTask(taskId);
    await client.getProcessingTaskIdentification(taskId);
    await client.retryProcessingTask(taskId, "processing-retry");
    await client.cancelProcessingTask(taskId, "processing-cancel");
    await client.listReviewCases({ cursor: "review-cursor", level: "ambiguous", inboxDirectoryId: inboxId, updatedBefore: "2026-07-23T08:00:00Z" });
    await client.getReviewCase(reviewCaseId);

    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/integrations/tmdb",
      "/api/v1/integrations/tmdb/connection-tests",
      "/api/v1/integrations/tmdb",
      "/api/v1/integrations/tmdb",
      `/api/v1/inbox-directories/${inboxId}/discovery-policy`,
      `/api/v1/inbox-directories/${inboxId}/discovery-policy`,
      "/api/v1/processing-tasks?cursor=processing-cursor",
      `/api/v1/processing-tasks/${taskId}`,
      `/api/v1/processing-tasks/${taskId}/identification`,
      `/api/v1/processing-tasks/${taskId}/attempts`,
      `/api/v1/processing-tasks/${taskId}/cancel`,
      `/api/v1/review-cases?cursor=review-cursor&decision_level=ambiguous&inbox_directory_id=${inboxId}&updated_before=2026-07-23T08%3A00%3A00Z`,
      `/api/v1/review-cases/${reviewCaseId}`,
    ]);
    expect((fetch.mock.calls[2][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "3" }));
    expect((fetch.mock.calls[3][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "4" }));
    expect((fetch.mock.calls[5][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "2" }));
    expect((fetch.mock.calls[9][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": "processing-retry" }));
    expect((fetch.mock.calls[10][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": "processing-cancel" }));
  });

  it("maps M4 downloader operations, filters, versions, and idempotency keys", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    const connectionId = "019f0000-0000-7000-8000-000000000061";
    const taskId = "019f0000-0000-7000-8000-000000000062";
    const input = {
      kind: "qbittorrent", display_name: "NAS qBittorrent", base_url: "http://nas.local:8080",
      username: "mediaflow", password: "candidate-password", enabled: true,
    };
    const m4 = client as unknown as {
      listDownloaderConnections(cursor?: string): Promise<unknown>;
      createDownloaderConnection(body: typeof input): Promise<unknown>;
      getDownloaderConnection(id: string): Promise<unknown>;
      updateDownloaderConnection(id: string, body: typeof input, configVersion: number): Promise<unknown>;
      deleteDownloaderConnection(id: string, configVersion: number): Promise<void>;
      testDownloaderConnection(body: typeof input): Promise<unknown>;
      listDownloadTasks(options: { cursor?: string; connectionId?: string; status?: string; query?: string }): Promise<unknown>;
      createDownloadTask(body: { connection_id: string; source: string; display_name: string }, idempotencyKey: string): Promise<unknown>;
      getDownloadTask(id: string): Promise<unknown>;
    };

    expect(client).toEqual(expect.objectContaining({
      listDownloaderConnections: expect.any(Function), createDownloaderConnection: expect.any(Function),
      getDownloaderConnection: expect.any(Function), updateDownloaderConnection: expect.any(Function),
      deleteDownloaderConnection: expect.any(Function), testDownloaderConnection: expect.any(Function),
      listDownloadTasks: expect.any(Function), createDownloadTask: expect.any(Function), getDownloadTask: expect.any(Function),
    }));

    await m4.listDownloaderConnections("connections-cursor");
    await m4.createDownloaderConnection(input);
    await m4.getDownloaderConnection(connectionId);
    await m4.updateDownloaderConnection(connectionId, input, 3);
    await m4.deleteDownloaderConnection(connectionId, 4);
    await m4.testDownloaderConnection(input);
    await m4.listDownloadTasks({ cursor: "tasks-cursor", connectionId, status: "monitoring", query: "Dune" });
    await m4.createDownloadTask({ connection_id: connectionId, source: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567", display_name: "Dune" }, "download-create-key");
    await m4.getDownloadTask(taskId);

    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/downloader-connections?cursor=connections-cursor",
      "/api/v1/downloader-connections",
      `/api/v1/downloader-connections/${connectionId}`,
      `/api/v1/downloader-connections/${connectionId}`,
      `/api/v1/downloader-connections/${connectionId}`,
      "/api/v1/downloader-connections/connection-tests",
      `/api/v1/download-tasks?cursor=tasks-cursor&connection_id=${connectionId}&status=monitoring&q=Dune`,
      "/api/v1/download-tasks",
      `/api/v1/download-tasks/${taskId}`,
    ]);
    expect((fetch.mock.calls[3][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "3" }));
    expect((fetch.mock.calls[4][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "4" }));
    expect((fetch.mock.calls[7][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": "download-create-key" }));
  });

  it("maps M4 source automation, event recovery, and enhancer operations", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    const sourceId = "019f0000-0000-7000-8000-000000000081";
    const eventId = "019f0000-0000-7000-8000-000000000082";
    const rss = {
      kind: "rss", display_name: "电影订阅", enabled: true,
      feed_url: "https://feeds.example.test/movies?token=candidate", downloader_connection_id: "019f0000-0000-7000-8000-000000000061",
      poll_interval_seconds: 900,
    } as const;
    const enhancer = {
      enabled: true, base_url: "http://127.0.0.1:11434", model: "qwen3:4b", timeout_ms: 3000,
    } as const;
    const m4 = client as unknown as {
      listAutomationSources(cursor?: string): Promise<unknown>;
      createAutomationSource(body: typeof rss): Promise<unknown>;
      getAutomationSource(id: string): Promise<unknown>;
      updateAutomationSource(id: string, body: typeof rss, version: number): Promise<unknown>;
      deleteAutomationSource(id: string, version: number): Promise<void>;
      testAutomationSource(body: typeof rss): Promise<unknown>;
      rotateAutomationWebhookSecret(id: string, version: number, idempotencyKey: string): Promise<unknown>;
      listAutomationEvents(options: { cursor?: string; sourceId?: string; status?: string; action?: string }): Promise<unknown>;
      getAutomationEvent(id: string): Promise<unknown>;
      retryAutomationEvent(id: string, idempotencyKey: string): Promise<unknown>;
      cancelAutomationEvent(id: string, idempotencyKey: string): Promise<unknown>;
      getIdentificationEnhancer(): Promise<unknown>;
      testIdentificationEnhancer(body: typeof enhancer): Promise<unknown>;
      putIdentificationEnhancer(body: typeof enhancer, version: number): Promise<unknown>;
    };

    expect(client).toEqual(expect.objectContaining({
      listAutomationSources: expect.any(Function), createAutomationSource: expect.any(Function),
      getAutomationSource: expect.any(Function), updateAutomationSource: expect.any(Function),
      deleteAutomationSource: expect.any(Function), testAutomationSource: expect.any(Function),
      rotateAutomationWebhookSecret: expect.any(Function), listAutomationEvents: expect.any(Function),
      getAutomationEvent: expect.any(Function), retryAutomationEvent: expect.any(Function),
      cancelAutomationEvent: expect.any(Function), getIdentificationEnhancer: expect.any(Function),
      testIdentificationEnhancer: expect.any(Function), putIdentificationEnhancer: expect.any(Function),
    }));

    await m4.listAutomationSources("source-cursor");
    await m4.createAutomationSource(rss);
    await m4.getAutomationSource(sourceId);
    await m4.updateAutomationSource(sourceId, rss, 2);
    await m4.deleteAutomationSource(sourceId, 3);
    await m4.testAutomationSource(rss);
    await m4.rotateAutomationWebhookSecret(sourceId, 4, "rotate-key");
    await m4.listAutomationEvents({ cursor: "event-cursor", sourceId, status: "retry-wait", action: "create-download" });
    await m4.getAutomationEvent(eventId);
    await m4.retryAutomationEvent(eventId, "retry-key");
    await m4.cancelAutomationEvent(eventId, "cancel-key");
    await m4.getIdentificationEnhancer();
    await m4.testIdentificationEnhancer(enhancer);
    await m4.putIdentificationEnhancer(enhancer, 5);

    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/automation-sources?cursor=source-cursor",
      "/api/v1/automation-sources",
      `/api/v1/automation-sources/${sourceId}`,
      `/api/v1/automation-sources/${sourceId}`,
      `/api/v1/automation-sources/${sourceId}`,
      "/api/v1/automation-sources/connection-tests",
      `/api/v1/automation-sources/${sourceId}/secret-rotations`,
      `/api/v1/automation-events?cursor=event-cursor&source_id=${sourceId}&status=retry-wait&action=create-download`,
      `/api/v1/automation-events/${eventId}`,
      `/api/v1/automation-events/${eventId}/retries`,
      `/api/v1/automation-events/${eventId}/cancellations`,
      "/api/v1/identification-enhancer",
      "/api/v1/identification-enhancer/connection-tests",
      "/api/v1/identification-enhancer",
    ]);
    for (const call of [3, 4, 6, 13]) {
      expect((fetch.mock.calls[call][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": String([2, 3, 4, 5][[3, 4, 6, 13].indexOf(call)]) }));
    }
    for (const [call, key] of [[6, "rotate-key"], [9, "retry-key"], [10, "cancel-key"]] as const) {
      expect((fetch.mock.calls[call][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": key }));
    }
  });

  it("maps M3 organization targets, versions, and processing-task commands", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => new Response(JSON.stringify({}), { status: 200, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    const targetId = "019f0000-0000-7000-8000-000000000071";
    const taskId = "019f0000-0000-7000-8000-000000000043";
    const input = {
      kind: "movie", display_name: "电影库", root_id: "media", relative_path: "Movies",
      operation: "copy", naming_pattern: "movie", nfo_policy: "generate-missing",
      automatic: true, enabled: true, rules: [],
    } as const;
    const organization = client as unknown as {
      listOrganizationTargets(cursor?: string): Promise<unknown>;
      preflightOrganizationTarget(body: { root_id: string; relative_path: string }): Promise<unknown>;
      createOrganizationTarget(body: typeof input): Promise<unknown>;
      getOrganizationTarget(id: string): Promise<unknown>;
      updateOrganizationTarget(id: string, body: typeof input, version: number): Promise<unknown>;
      deleteOrganizationTarget(id: string, version: number): Promise<void>;
      getProcessingTaskOrganization(id: string): Promise<unknown>;
      recalculateProcessingTaskOrganization(id: string, idempotencyKey: string): Promise<unknown>;
      executeProcessingTaskOrganization(id: string, planVersion: number, idempotencyKey: string): Promise<unknown>;
      rollbackProcessingTaskOrganization(id: string, resultVersion: number, idempotencyKey: string): Promise<unknown>;
    };

    expect(client).toEqual(expect.objectContaining({
      listOrganizationTargets: expect.any(Function), preflightOrganizationTarget: expect.any(Function),
      createOrganizationTarget: expect.any(Function), getOrganizationTarget: expect.any(Function),
      updateOrganizationTarget: expect.any(Function), deleteOrganizationTarget: expect.any(Function),
      getProcessingTaskOrganization: expect.any(Function), recalculateProcessingTaskOrganization: expect.any(Function),
      executeProcessingTaskOrganization: expect.any(Function), rollbackProcessingTaskOrganization: expect.any(Function),
    }));

    await organization.listOrganizationTargets("targets-cursor");
    await organization.preflightOrganizationTarget({ root_id: "media", relative_path: "Movies" });
    await organization.createOrganizationTarget(input);
    await organization.getOrganizationTarget(targetId);
    await organization.updateOrganizationTarget(targetId, input, 3);
    await organization.deleteOrganizationTarget(targetId, 4);
    await organization.getProcessingTaskOrganization(taskId);
    await organization.recalculateProcessingTaskOrganization(taskId, "recalculate-key");
    await organization.executeProcessingTaskOrganization(taskId, 2, "execute-key");
    await organization.rollbackProcessingTaskOrganization(taskId, 5, "rollback-key");

    expect(fetch.mock.calls.map(([path]) => path)).toEqual([
      "/api/v1/organization-targets?cursor=targets-cursor",
      "/api/v1/organization-targets/preflights",
      "/api/v1/organization-targets",
      `/api/v1/organization-targets/${targetId}`,
      `/api/v1/organization-targets/${targetId}`,
      `/api/v1/organization-targets/${targetId}`,
      `/api/v1/processing-tasks/${taskId}/organization`,
      `/api/v1/processing-tasks/${taskId}/organization/recalculations`,
      `/api/v1/processing-tasks/${taskId}/organization/executions`,
      `/api/v1/processing-tasks/${taskId}/organization/rollbacks`,
    ]);
    expect((fetch.mock.calls[4][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "3" }));
    expect((fetch.mock.calls[5][1] as RequestInit).headers).toEqual(expect.objectContaining({ "If-Match": "4" }));
    for (const [call, key] of [[7, "recalculate-key"], [8, "execute-key"], [9, "rollback-key"]] as const) {
      expect((fetch.mock.calls[call][1] as RequestInit).headers).toEqual(expect.objectContaining({ "Idempotency-Key": key }));
    }
    expect(JSON.parse((fetch.mock.calls[8][1] as RequestInit).body as string)).toEqual({ plan_version: 2 });
    expect(JSON.parse((fetch.mock.calls[9][1] as RequestInit).body as string)).toEqual({ result_version: 5 });
  });

  it("normalizes malformed and non-envelope error responses", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(new Response("{", { status: 500, headers: { "content-type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: { code: "internal.error", message: "nope", request_id: "not-a-uuid" } }), { status: 403, headers: { "content-type": "application/json" } }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ error: { code: "internal.error", message: "nope", request_id: "019f0000-0000-7000-8000-000000000001", unsafe: true } }), { status: 500, headers: { "content-type": "application/json" } }));
    const client = createMediaFlowClient({ fetch });
    await expect(client.getBootstrapStatus()).rejects.toEqual(expect.objectContaining({ name: "MediaFlowApiError", status: 500, body: undefined }));
    await expect(client.getBootstrapStatus()).rejects.toEqual(expect.objectContaining({ name: "MediaFlowApiError", status: 403, body: undefined }));
    await expect(client.getBootstrapStatus()).rejects.toEqual(expect.objectContaining({ name: "MediaFlowApiError", status: 500, body: undefined }));
  });

  it("retains allow-listed M3 integration failures without exposing arbitrary fields", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(
      new Response(JSON.stringify({ error: { code: "provider.unavailable", message: "retry later", request_id: "019f0000-0000-7000-8000-000000000001" } }), {
        status: 503,
        headers: { "content-type": "application/json" },
      }),
    );
    const client = createMediaFlowClient({ fetch });
    await expect(client.getTmdbIntegration()).rejects.toEqual(expect.objectContaining({
      name: "MediaFlowApiError",
      status: 503,
      body: { error: { code: "provider.unavailable", message: "retry later", request_id: "019f0000-0000-7000-8000-000000000001" } },
    }));
  });

  it("declares a stable public package entrypoint", () => {
    const manifest = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
    expect(manifest.types).toBe("./src/index.ts");
    expect(manifest.exports).toEqual({ ".": { types: "./src/index.ts", default: "./src/index.ts" } });
  });

  it("resolves its public entrypoint by package name", () => {
    expect(createPublicClient).toBe(createMediaFlowClient);
  });
});
