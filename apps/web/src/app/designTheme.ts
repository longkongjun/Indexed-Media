/**
 * Indexed-Design 主题桥：把宿主主题三态解析为 `@indexed-design/ui-vue` 需要的显式浅/暗 mode。
 *
 * 主题唯一真源仍是 `./theme` 写入的 root `data-theme` 属性与 `prefers-color-scheme`：
 * `system` 不写属性，由本模块读系统媒体查询解析。本模块不新增持久化或第二份主题状态，
 * 只在属性变化（`mfd-theme-change`）与系统偏好变化时保持响应。皮肤维度（`data-skin`，
 * aurora/sepia）本批不映射到 Indexed 品牌，缺口见 docs/development/indexed-design-adoption.md。
 */
import { computed, onBeforeUnmount, onMounted, ref, type Ref } from "vue";
import type { IndexedThemeMode } from "@indexed-design/ui-vue";

const SYSTEM_QUERY = "(prefers-color-scheme: dark)";

function readExplicitMode(): "light" | "dark" | null {
  const value = document.documentElement.dataset.theme;
  return value === "light" || value === "dark" ? value : null;
}

function readSystemDark(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(SYSTEM_QUERY).matches;
}

/** 解析后的设计主题 mode；显式选择优先，跟随系统时由媒体查询决定。 */
export function useDesignThemeMode(): Readonly<Ref<IndexedThemeMode>> {
  const explicit = ref<"light" | "dark" | null>(readExplicitMode());
  const systemDark = ref(readSystemDark());
  const onRootThemeChange = (): void => { explicit.value = readExplicitMode(); };
  const onSystemChange = (event: MediaQueryListEvent): void => { systemDark.value = event.matches; };
  const media = typeof window.matchMedia === "function" ? window.matchMedia(SYSTEM_QUERY) : null;

  onMounted(() => {
    document.documentElement.addEventListener("mfd-theme-change", onRootThemeChange);
    media?.addEventListener("change", onSystemChange);
  });
  onBeforeUnmount(() => {
    document.documentElement.removeEventListener("mfd-theme-change", onRootThemeChange);
    media?.removeEventListener("change", onSystemChange);
  });

  return computed(() => explicit.value ?? (systemDark.value ? "dark" : "light"));
}
