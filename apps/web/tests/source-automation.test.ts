import type {
  AutomationEvent,
  AutomationSource,
  IdentificationEnhancer,
  MediaFlowClient,
} from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useAutomationEvents } from "../src/features/source-automation/useAutomationEvents";
import { useAutomationSources } from "../src/features/source-automation/useAutomationSources";
import { useIdentificationEnhancer } from "../src/features/source-automation/useIdentificationEnhancer";
import AutomationSourcesView from "../src/views/AutomationSourcesView.vue";

const source: AutomationSource = {
  id: "019f0000-0000-7000-8000-000000000070",
  kind: "rss",
  display_name: "Release feed",
  enabled: true,
  downloader_connection_id: "019f0000-0000-7000-8000-000000000060",
  inbox_directory_id: null,
  endpoint_summary: "feeds.example.test",
  poll_interval_seconds: 900,
  allowed_actions: [],
  secret_fingerprint: null,
  config_version: 2,
  health: "healthy",
  checked_at: "2026-07-24T08:00:00Z",
  failure_code: null,
  projection_version: 3,
  updated_at: "2026-07-24T08:00:00Z",
};

const event: AutomationEvent = {
  id: "019f0000-0000-7000-8000-000000000071",
  source_id: source.id,
  source_display_name: source.display_name,
  action: "reconcile-inbox",
  status: "completed",
  downstream_kind: "reconcile-request",
  downstream_id: "019f0000-0000-7000-8000-000000000072",
  result_count: 0,
  failure_code: null,
  attempt_count: 1,
  retry_at: null,
  allowed_actions: [],
  projection_version: 2,
  created_at: "2026-07-24T08:00:00Z",
  updated_at: "2026-07-24T08:01:00Z",
};

const enhancer: IdentificationEnhancer = {
  kind: "ollama",
  enabled: false,
  endpoint_summary: "http://127.0.0.1:11434",
  model: "qwen3:4b",
  timeout_ms: 3_000,
  config_version: 1,
  health: "degraded",
  checked_at: null,
  fallback_code: "automation.source-disabled",
  projection_version: 1,
  updated_at: "2026-07-24T08:00:00Z",
};

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listAutomationSources: vi.fn(async () => ({ items: [source], next_cursor: null })),
    createAutomationSource: vi.fn(async () => source),
    getAutomationSource: vi.fn(async () => source),
    updateAutomationSource: vi.fn(async () => source),
    deleteAutomationSource: vi.fn(async () => undefined),
    testAutomationSource: vi.fn(async () => ({
      reachable: true, health: "healthy", detected_format: "rss-2.0",
      item_count: 4, ignored_item_count: 1, failure_code: null,
      checked_at: "2026-07-24T08:00:00Z",
    })),
    rotateAutomationWebhookSecret: vi.fn(async () => ({ source, secret: "one-time-secret" })),
    listAutomationEvents: vi.fn(async () => ({ items: [event], next_cursor: null })),
    getAutomationEvent: vi.fn(async () => event),
    retryAutomationEvent: vi.fn(async () => event),
    cancelAutomationEvent: vi.fn(async () => event),
    getIdentificationEnhancer: vi.fn(async () => enhancer),
    testIdentificationEnhancer: vi.fn(async () => ({
      reachable: false, health: "unavailable", adapter_version: "ollama-v1",
      model_available: false, fallback_code: "provider.timeout",
      checked_at: "2026-07-24T08:00:00Z",
    })),
    putIdentificationEnhancer: vi.fn(async () => enhancer),
    listDownloaderConnections: vi.fn(async () => ({ items: [], next_cursor: null })),
    listDeploymentRoots: vi.fn(async () => ({ items: [], next_cursor: null })),
    listInboxDirectories: vi.fn(async () => ({ items: [], next_cursor: null })),
    setCsrfToken: vi.fn(),
    ...overrides,
  } as unknown as MediaFlowClient;
}

describe("source automation administration", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("tests RSS before save and clears sensitive input on a version conflict", async () => {
    const api = client({
      updateAutomationSource: vi.fn(async () => {
        throw new MediaFlowApiError(409, {
          error: { code: "request.conflict", message: "unsafe upstream", request_id: source.id },
        });
      }),
    });
    const feature = useAutomationSources(api);
    Object.assign(feature.form, {
      kind: "rss", displayName: "Draft feed", enabled: true,
      feedUrl: "https://private.example.test/feed?token=SECRET",
      downloaderConnectionId: source.downloader_connection_id ?? "", pollIntervalSeconds: 900,
    });
    await feature.testCandidate();
    expect(api.testAutomationSource).toHaveBeenCalledTimes(1);
    expect(api.createAutomationSource).not.toHaveBeenCalled();
    expect(feature.canSave.value).toBe(true);
    feature.selected.value = source;
    await feature.update();
    expect(feature.form.displayName).toBe("Draft feed");
    expect(feature.form.feedUrl).toBe("");
    expect(feature.conflictProjection.value?.config_version).toBe(2);
    expect(feature.formError.value?.message).not.toContain("unsafe upstream");
  });

  it("keeps a Webhook secret only for the current copy-confirm interaction", async () => {
    const receiptSource = { ...source, kind: "webhook" as const, endpoint_summary: "/api/v1/source-webhooks/…/events" };
    const api = client({
      createAutomationSource: vi.fn(async () => ({
        source: receiptSource,
        secret: "SECRET_MUST_DISAPPEAR",
      })),
    });
    const feature = useAutomationSources(api);
    Object.assign(feature.form, {
      kind: "webhook", displayName: "Webhook", enabled: true,
      allowedActions: ["download.create"],
    });
    await feature.save();
    expect(feature.secretReceipt.value?.secret).toBe("SECRET_MUST_DISAPPEAR");
    expect(feature.canLeaveSecret.value).toBe(false);
    feature.secretCopied.value = true;
    expect(feature.canLeaveSecret.value).toBe(true);
    feature.dismissSecret();
    expect(feature.secretReceipt.value).toBeNull();
    expect(JSON.stringify(feature.items.value)).not.toContain("SECRET_MUST_DISAPPEAR");
  });

  it("retains stale projections and disables every write while offline", async () => {
    const list = vi.fn()
      .mockResolvedValueOnce({ items: [source], next_cursor: null })
      .mockRejectedValueOnce(new TypeError("offline"));
    const feature = useAutomationSources(client({ listAutomationSources: list }));
    await feature.load();
    await feature.load();
    expect(feature.items.value).toEqual([source]);
    expect(feature.state.value).toMatchObject({ kind: "offline", stale: true });
    expect(feature.canWrite.value).toBe(false);
  });

  it("keeps other source cards usable when one source refresh fails", async () => {
    const other = { ...source, id: "019f0000-0000-7000-8000-000000000073", display_name: "Other feed" };
    const api = client({
      listAutomationSources: vi.fn(async () => ({ items: [source, other], next_cursor: null })),
      getAutomationSource: vi.fn(async () => {
        throw new MediaFlowApiError(503, {
          error: { code: "provider.unavailable", message: "unsafe upstream detail", request_id: source.id },
        });
      }),
    });
    const feature = useAutomationSources(api);
    await feature.load();
    await feature.refreshSource(source.id);
    expect(feature.state.value).toMatchObject({ kind: "content" });
    expect(feature.items.value).toEqual([source, other]);
    expect(feature.sourceErrors[source.id]).toBe("此来源刷新失败，保留上次投影");
    expect(feature.sourceErrors[source.id]).not.toContain("unsafe upstream detail");
  });

  it("uses server event capabilities and explains a zero-result reconcile", async () => {
    const feature = useAutomationEvents(client());
    await feature.load();
    expect(feature.resultText(event)).toBe("对账完成，未发现新文件");
    expect(feature.canRetry(event)).toBe(false);
    expect(feature.canCancel({ ...event, status: "running", allowed_actions: ["cancel"] })).toBe(true);
    await feature.retry({ ...event, status: "failed", allowed_actions: ["retry"] });
    expect(feature.items.value[0]?.id).toBe(event.id);
  });

  it("keeps deterministic identification visibly available through enhancer fallback", async () => {
    const feature = useIdentificationEnhancer(client());
    await feature.load();
    await feature.testCandidate();
    expect(feature.fallbackMessage.value).toContain("确定性识别仍在运行");
    expect(feature.testResult.value?.fallback_code).toBe("provider.timeout");
    expect(feature.canWrite.value).toBe(true);
    feature.form.enabled = true;
    expect(feature.canSave.value).toBe(false);
  });

  it("renders four explicit creation entries and independent empty states", async () => {
    const api = client({
      listAutomationSources: vi.fn(async () => ({ items: [], next_cursor: null })),
      listAutomationEvents: vi.fn(async () => ({ items: [], next_cursor: null })),
    });
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: "/automation/sources", component: AutomationSourcesView },
      { path: "/automation/sources/:id", name: "automation-source", component: { template: "<main>source</main>" } },
      { path: "/automation/events/:id", name: "automation-event", component: { template: "<main>event</main>" } },
    ] });
    await router.push("/automation/sources");
    const wrapper = mount(AutomationSourcesView, {
      global: { plugins: [router], provide: { [identityClientKey as symbol]: api } },
    });
    await flushPromises();
    expect(wrapper.text()).toContain("尚未配置自动来源");
    expect(wrapper.text()).toContain("来源尚无事件");
    for (const label of ["RSS / Atom", "签名 Webhook", "下载完成映射", "本地识别增强"]) {
      expect(wrapper.text()).toContain(label);
    }
    expect(wrapper.text()).toContain("默认关闭");
    wrapper.unmount();
  });
});
