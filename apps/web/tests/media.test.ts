import type { MediaFlowClient, MediaItemDetail, MediaItemPage } from "@mediaflow/api-client-ts";
import { flushPromises, mount } from "@vue/test-utils";
import { createPinia, setActivePinia } from "pinia";
import { describe, expect, it, vi } from "vitest";
import { createMemoryHistory, createRouter } from "vue-router";
import { identityClientKey } from "../src/app/client";
import MediaArtwork from "../src/components/MediaArtwork.vue";
import { useMediaItems } from "../src/features/media/useMediaItems";
import MediaDetailView from "../src/views/MediaDetailView.vue";
import MediaListView from "../src/views/MediaListView.vue";

const itemId = "019f0000-0000-7000-8000-000000000050";
const libraryId = "019f0000-0000-7000-8000-000000000051";
const item = {
  id: itemId, type: "movie" as const, library_id: libraryId, title: "沙丘", year: 2021,
  local_status: "partial" as const, artwork_ref: null, updated_at: "2026-07-23T10:00:00Z",
};
const page: MediaItemPage = { items: [item], next_cursor: "media-next" };
const detail: MediaItemDetail = {
  item,
  metadata: [
    { field: "overview", value: null, state: "missing", source_type: null, source_id: null, source_version: null },
    { field: "title", value: "沙丘", state: "present", source_type: "tmdb", source_id: "438631", source_version: "zh-CN" },
  ],
  versions: [{ id: "019f0000-0000-7000-8000-000000000052", label: "4K", files: [{ id: "019f0000-0000-7000-8000-000000000053", source_relative_path: "incoming/Dune.2021.mkv", current_relative_path: "movies/Dune (2021)/Dune (2021).mkv", size_bytes: 1024 }] }],
  children: [{ id: "019f0000-0000-7000-8000-000000000054", parent_id: null, type: "generic-video-item", title: "花絮", ordinal: 1, versions: [] }],
  nfo_status: "failed",
  related_task_ids: ["019f0000-0000-7000-8000-000000000043"],
};

function api(overrides: Partial<MediaFlowClient> = {}): MediaFlowClient {
  return {
    listMediaItems: vi.fn(async () => page),
    getMediaItem: vi.fn(async () => detail),
    ...overrides,
  } as MediaFlowClient;
}

function router() {
  return createRouter({ history: createMemoryHistory(), routes: [
    { path: "/media", name: "media", component: MediaListView },
    { path: "/media/:id", name: "media-item", component: MediaDetailView },
    { path: "/tasks", name: "tasks", component: { template: "<main>tasks</main>" } },
    { path: "/tasks/:id", name: "task", component: { template: "<main>task</main>" } },
    { path: "/login", name: "login", component: { template: "<main>login</main>" } },
  ] });
}

describe("formal media catalog", () => {
  it("sends every canonical route filter to Core without client filtering", async () => {
    const client = api();
    const feature = useMediaItems(client, { type: "movie", libraryId, localStatus: "partial", query: "%_沙丘", cursor: "cursor-a" });
    await feature.load();
    expect(client.listMediaItems).toHaveBeenCalledWith({ type: "movie", libraryId, localStatus: "partial", query: "%_沙丘", cursor: "cursor-a" });
    expect(feature.items.value).toEqual(page.items);
    expect(feature.nextCursor.value).toBe("media-next");
  });

  it("shows an honest empty catalog with exactly one task link and no ReviewCase dependency", async () => {
    const client = api({ listMediaItems: vi.fn(async () => ({ items: [], next_cursor: null })) });
    const pinia = createPinia(); setActivePinia(pinia); const appRouter = router(); await appRouter.push("/media");
    const wrapper = mount(MediaListView, { global: { plugins: [pinia, appRouter], provide: { [identityClientKey as symbol]: client } } });
    await flushPromises();
    expect(wrapper.text()).toContain("尚无已整理媒体");
    expect(wrapper.findAll('a[href="/tasks"]')).toHaveLength(1);
    expect(wrapper.findAll("select")[0]!.findAll("option").map((option) => option.attributes("value"))).toEqual(["", "movie", "series", "generic-video"]);
    expect(client.listReviewCases).toBeUndefined();
    wrapper.unmount();
  });

  it("never turns an opaque artwork reference into a remote image URL", () => {
    const missing = mount(MediaArtwork, { props: { artwork: null, title: "沙丘" } });
    expect(missing.text()).toContain("暂无本地封面");
    expect(missing.find("img").exists()).toBe(false);
    const local = mount(MediaArtwork, { props: { artwork: { id: itemId, kind: "poster", state: "available" }, title: "沙丘" } });
    expect(local.text()).toContain("本地封面已登记");
    expect(local.html()).not.toMatch(/https?:\/\//i);
    expect(local.find("img").exists()).toBe(false);
  });

  it("renders partial metadata, failed NFO, local files and bounded hierarchy without Jellyfin fields", async () => {
    const client = api();
    const pinia = createPinia(); setActivePinia(pinia); const appRouter = router(); await appRouter.push(`/media/${itemId}`);
    const wrapper = mount(MediaDetailView, { global: { plugins: [pinia, appRouter], provide: { [identityClientKey as symbol]: client } } });
    await flushPromises();
    expect(wrapper.text()).toContain("元数据缺失");
    expect(wrapper.text()).toContain("NFO：生成失败");
    expect(wrapper.text()).toContain("movies/Dune (2021)/Dune (2021).mkv");
    expect(wrapper.text()).toContain("花絮");
    expect(wrapper.text()).not.toMatch(/Jellyfin|remote.*image|http:\/\//i);
    wrapper.unmount();
  });
});
