import type { MediaFlowClient, NotificationPage } from "@mediaflow/api-client-ts";
import { describe, expect, it, vi } from "vitest";
import { useNotifications } from "../src/features/notifications/useNotifications";

function page(id: string, cursor: string | null = null): NotificationPage {
  return {
    items: [{ id, kind: "system.notice", title: id, body: "通知", task_id: null, created_at: "2026-10-27T00:00:00.000001Z", read_at: null }],
    next_cursor: cursor,
    unread_count: 2,
  };
}

describe("notification pagination", () => {
  it("keeps the selected unread filter on every appended page and resets it on the next load", async () => {
    const list = vi.fn<MediaFlowClient["listNotifications"]>()
      .mockResolvedValueOnce(page("unread-first", "unread-next"))
      .mockResolvedValueOnce(page("unread-second"))
      .mockResolvedValueOnce(page("all-first", "all-next"))
      .mockResolvedValueOnce(page("all-second"));
    const feature = useNotifications({ listNotifications: list } as unknown as MediaFlowClient);
    await feature.load({ unreadOnly: true });
    await feature.loadMore();
    expect(list.mock.calls[1]?.[0]).toEqual({ cursor: "unread-next", unreadOnly: true });
    expect(feature.items.value.map((item) => item.id)).toEqual(["unread-first", "unread-second"]);
    await feature.load({ unreadOnly: false });
    await feature.loadMore();
    expect(list.mock.calls[3]?.[0]).toEqual({ cursor: "all-next", unreadOnly: false });
    expect(feature.items.value.map((item) => item.id)).toEqual(["all-first", "all-second"]);
  });

  it("does not pair an old cursor with a new filter while its first page is pending", async () => {
    let resolveFirst: ((value: NotificationPage) => void) | undefined;
    const delayed = new Promise<NotificationPage>((resolve) => { resolveFirst = resolve; });
    const list = vi.fn<MediaFlowClient["listNotifications"]>()
      .mockResolvedValueOnce(page("all-first", "all-next"))
      .mockReturnValueOnce(delayed)
      .mockResolvedValueOnce(page("unexpected-unread-second"));
    const feature = useNotifications({ listNotifications: list } as unknown as MediaFlowClient);
    await feature.load({ unreadOnly: false });
    const changedFilter = feature.load({ unreadOnly: true });
    expect(feature.items.value.map((item) => item.id)).toEqual(["all-first"]);
    expect(feature.state.value).toEqual({ kind: "loading", stale: true });
    await feature.loadMore();
    resolveFirst?.(page("unread-first", "unread-next"));
    await changedFilter;
    expect(list).toHaveBeenCalledTimes(2);
    expect(feature.items.value.map((item) => item.id)).toEqual(["unread-first"]);
    expect(feature.nextCursor.value).toBe("unread-next");
    expect(feature.state.value).toEqual({ kind: "content", stale: false });
  });

  it("does not reuse the old cursor after a filter's first-page request fails", async () => {
    const list = vi.fn<MediaFlowClient["listNotifications"]>()
      .mockResolvedValueOnce(page("all-first", "all-next"))
      .mockRejectedValueOnce(new TypeError("new filter unavailable"))
      .mockResolvedValueOnce(page("unexpected-unread-second"));
    const feature = useNotifications({ listNotifications: list } as unknown as MediaFlowClient);
    await feature.load({ unreadOnly: false });
    await feature.load({ unreadOnly: true });
    await feature.loadMore();
    expect(list).toHaveBeenCalledTimes(2);
    expect(feature.items.value.map((item) => item.id)).toEqual(["all-first"]);
    expect(feature.nextCursor.value).toBeNull();
    expect(feature.state.value).toEqual({ kind: "offline", stale: true });
  });

  it("ignores a delayed old page after switching the unread filter", async () => {
    let resolveOld: ((value: NotificationPage) => void) | undefined;
    const delayed = new Promise<NotificationPage>((resolve) => { resolveOld = resolve; });
    const list = vi.fn<MediaFlowClient["listNotifications"]>()
      .mockResolvedValueOnce(page("all-first", "all-next"))
      .mockReturnValueOnce(delayed)
      .mockResolvedValueOnce(page("unread-first", "unread-next"));
    const feature = useNotifications({ listNotifications: list } as unknown as MediaFlowClient);
    await feature.load({ unreadOnly: false });
    const oldPage = feature.loadMore();
    await feature.load({ unreadOnly: true });
    resolveOld?.(page("stale-all-second"));
    await oldPage;
    expect(feature.items.value.map((item) => item.id)).toEqual(["unread-first"]);
    expect(feature.nextCursor.value).toBe("unread-next");
  });

  it.each(["load", "loadMore"] as const)("ignores a delayed %s failure hook after a new filter load", async (operation) => {
    let releaseFailure: (() => void) | undefined;
    const handled = new Promise<void>((resolve) => { releaseFailure = resolve; });
    let enteredFailure: (() => void) | undefined;
    const entered = new Promise<void>((resolve) => { enteredFailure = resolve; });
    const onFailure = vi.fn(() => { enteredFailure?.(); return handled; });
    const list = vi.fn<MediaFlowClient["listNotifications"]>()
      .mockResolvedValueOnce(page("all-first", "all-next"))
      .mockRejectedValueOnce(new TypeError("old page unavailable"))
      .mockResolvedValueOnce(page("unread-first", "unread-next"));
    const feature = useNotifications({ listNotifications: list } as unknown as MediaFlowClient, onFailure);
    await feature.load({ unreadOnly: false });
    const oldRequest = feature[operation]();
    await entered;
    await feature.load({ unreadOnly: true });
    releaseFailure?.();
    await oldRequest;
    expect(feature.items.value.map((item) => item.id)).toEqual(["unread-first"]);
    expect(feature.state.value).toEqual({ kind: "content", stale: false });
    expect(feature.errorMessage.value).toBe("");
    expect(feature.nextCursor.value).toBe("unread-next");
  });
});
