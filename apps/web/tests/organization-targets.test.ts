import type {
  MediaFlowClient,
  OrganizationTarget,
  OrganizationTargetPreflight,
} from "@mediaflow/api-client-ts";
import { MediaFlowApiError } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import { useOrganizationTargets } from "../src/features/organization-targets/useOrganizationTargets";
import OrganizationTargetsView from "../src/views/OrganizationTargetsView.vue";

const targetId = "019f0000-0000-7000-8000-000000000071";
const target = (version = 1): OrganizationTarget => ({
  id: targetId,
  kind: "movie",
  display_name: "电影库",
  root_id: "library",
  relative_path: "Movies",
  operation: "copy",
  naming_pattern: "movie",
  nfo_policy: "generate-missing",
  automatic: true,
  enabled: true,
  rules: [{ media_kind: "movie", inbox_directory_id: null, explicit_tag: "trusted", enabled: true }],
  config_version: version,
  updated_at: "2026-07-24T08:00:00Z",
});

const accepted: OrganizationTargetPreflight = {
  root_id: "library",
  relative_path: "Movies",
  writable: true,
  overlaps_existing: false,
  same_filesystem_hint: true,
  failure_code: null,
};

function client(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listDeploymentRoots: vi.fn(async () => ({
      items: [
        { id: "incoming", label: "只读收件", access: "read-only" as const },
        { id: "library", label: "媒体库", access: "read-write" as const },
      ],
      next_cursor: null,
    })),
    listOrganizationTargets: vi.fn(async () => ({ items: [target()], next_cursor: null })),
    preflightOrganizationTarget: vi.fn(async () => accepted),
    createOrganizationTarget: vi.fn(async () => target()),
    getOrganizationTarget: vi.fn(async () => target()),
    updateOrganizationTarget: vi.fn(async () => target(2)),
    deleteOrganizationTarget: vi.fn(async () => undefined),
    setCsrfToken: vi.fn(),
    ...overrides,
  } as unknown as MediaFlowClient;
}

describe("organization target management", () => {
  beforeEach(() => setActivePinia(createPinia()));

  it("requires a successful current preflight before persisting the bounded target", async () => {
    const api = client();
    const feature = useOrganizationTargets(api);
    await feature.load();
    Object.assign(feature.form, {
      kind: "movie",
      displayName: "电影库",
      rootId: "library",
      relativePath: "Movies/../Movies",
      operation: "copy",
      namingPattern: "movie",
      nfoPolicy: "generate-missing",
      automatic: true,
      enabled: true,
    });
    feature.form.rules.splice(0, feature.form.rules.length, {
      media_kind: "movie", inbox_directory_id: null, explicit_tag: "trusted", enabled: true,
    });

    expect(await feature.save()).toBeNull();
    expect(api.createOrganizationTarget).not.toHaveBeenCalled();
    expect(feature.formError.value?.field).toBe("relative-path");
    expect(await feature.preflight()).toEqual(accepted);
    expect(feature.form.relativePath).toBe("Movies");
    expect(await feature.save()).toEqual(target());
    expect(api.preflightOrganizationTarget).toHaveBeenCalledWith({
      root_id: "library", relative_path: "Movies/../Movies",
    });
    expect(api.createOrganizationTarget).toHaveBeenCalledWith({
      kind: "movie", display_name: "电影库", root_id: "library", relative_path: "Movies",
      operation: "copy", naming_pattern: "movie", nfo_policy: "generate-missing",
      automatic: true, enabled: true,
      rules: [{ media_kind: "movie", inbox_directory_id: null, explicit_tag: "trusted", enabled: true }],
    });
  });

  it("enables save when preflight accepts an already-normalized path unchanged", async () => {
    const api = client();
    const feature = useOrganizationTargets(api);
    await feature.load();
    feature.form.displayName = "电影库";
    feature.form.rootId = "library";
    feature.form.relativePath = "Movies";
    expect(feature.preflightMatches.value).toBe(false);

    expect(await feature.preflight()).toEqual(accepted);

    expect(feature.preflightMatches.value).toBe(true);
    expect(await feature.save()).toEqual(target());
  });

  it.each([
    ["organization.root-read-only", "root-id"],
    ["organization.target-overlap", "relative-path"],
  ] as const)("maps %s preflight failures to a focusable field without saving", async (failureCode, field) => {
    const api = client({
      preflightOrganizationTarget: vi.fn(async () => ({
        ...accepted,
        writable: failureCode !== "organization.root-read-only",
        overlaps_existing: failureCode === "organization.target-overlap",
        failure_code: failureCode,
      })),
    });
    const feature = useOrganizationTargets(api);
    feature.form.rootId = "library";
    feature.form.relativePath = "Movies";
    expect(await feature.preflight()).toBeNull();
    expect(feature.formError.value?.field).toBe(field);
    expect(await feature.save()).toBeNull();
    expect(api.createOrganizationTarget).not.toHaveBeenCalled();
  });

  it("preserves a non-sensitive draft and refreshes latest facts after a stale update", async () => {
    const get = vi.fn().mockResolvedValueOnce(target()).mockResolvedValueOnce(target(2));
    const api = client({
      getOrganizationTarget: get,
      updateOrganizationTarget: vi.fn(async () => {
        throw new MediaFlowApiError(409, {
          error: { code: "request.conflict", message: "/private/host/SECRET", request_id: targetId },
        });
      }),
    });
    const feature = useOrganizationTargets(api);
    await feature.loadDetail(targetId);
    feature.form.displayName = "我的未提交草稿";
    await feature.preflight();
    expect(await feature.update()).toBeNull();
    expect(feature.form.displayName).toBe("我的未提交草稿");
    expect(feature.conflictLatest.value?.config_version).toBe(2);
    expect(feature.formError.value?.message).toContain("版本 2");
    expect(feature.formError.value?.message).not.toContain("SECRET");
    expect(api.updateOrganizationTarget).toHaveBeenCalledWith(
      targetId,
      expect.objectContaining({ display_name: "我的未提交草稿" }),
      1,
    );
  });

  it("keeps the last safe list offline and renders only writable roots and bounded rule summaries", async () => {
    const list = vi.fn()
      .mockResolvedValueOnce({ items: [target()], next_cursor: null })
      .mockRejectedValueOnce(new TypeError("offline"));
    const api = client({ listOrganizationTargets: list });
    const feature = useOrganizationTargets(api);
    await feature.load();
    await feature.load();
    expect(feature.items.value).toEqual([target()]);
    expect(feature.state.value).toEqual({ kind: "offline", stale: true });
    expect(feature.canWrite.value).toBe(false);

    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/organization/targets", component: OrganizationTargetsView },
        { path: "/organization/targets/:id", name: "organization-target", component: { template: "<main>detail</main>" } },
        { path: "/login", name: "login", component: { template: "<main>login</main>" } },
      ],
    });
    await router.push("/organization/targets");
    const wrapper = mount(OrganizationTargetsView, {
      global: { plugins: [router], provide: { [identityClientKey as symbol]: client() } },
    });
    await flushPromises();
    expect(wrapper.get("#root-id").text()).toContain("媒体库");
    expect(wrapper.get("#root-id").text()).not.toContain("只读收件");
    expect(wrapper.text()).toContain("自动执行：已启用");
    expect(wrapper.text()).toContain("规则 1 条");
    expect(wrapper.text()).not.toMatch(/container_path|\/private\/|SECRET/);
    wrapper.unmount();
  });
});
