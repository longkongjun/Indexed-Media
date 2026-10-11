// 风格指南站入口：主题控制器（theme-switching 模式 §1-2）+ hash 路由。
// 零依赖：原生 ES Modules；数据 fetch 本地 assets/tokens.json 与 fixtures。

import { renderTokensView } from "./views/tokens.mjs";
import { renderComponentsView } from "./views/components.mjs";
import { renderPatternsView } from "./views/patterns.mjs";
import { icon } from "./demo-kit.mjs";
import { fetchTokens, fixtures } from "./fixtures.mjs";

/* ---------- 主题三态 + 皮肤二维（docs/skins.md） ---------- */

const THEME_KEY = "mfd-theme";
const SKIN_KEY = "mfd-skin";

function applyThemeChoice(choice) {
  const root = document.documentElement;
  if (choice === "light" || choice === "dark") root.dataset.theme = choice;
  else delete root.dataset.theme; // 跟随系统：交给 prefers-color-scheme
  try { localStorage.setItem(THEME_KEY, choice); } catch { /* 隐私模式等：仅本次会话生效 */ }
  for (const btn of document.querySelectorAll("[data-theme-choice]")) {
    btn.setAttribute("aria-checked", String(btn.dataset.themeChoice === choice));
  }
  document.documentElement.dispatchEvent(new CustomEvent("mfd-theme-change", { detail: { theme: choice } }));
}

function applySkinChoice(skin) {
  const root = document.documentElement;
  if (skin && skin !== "evergreen") root.dataset.skin = skin;
  else delete root.dataset.skin; // 基线皮肤 = 无属性
  try { localStorage.setItem(SKIN_KEY, skin); } catch { /* 同上 */ }
  for (const btn of document.querySelectorAll("[data-skin-choice]")) {
    btn.setAttribute("aria-checked", String(btn.dataset.skinChoice === skin));
  }
  document.documentElement.dispatchEvent(new CustomEvent("mfd-skin-change", { detail: { skin } }));
}

function initTheme() {
  let saved = "system", skin = "evergreen";
  try {
    saved = localStorage.getItem(THEME_KEY) || "system";
    skin = localStorage.getItem(SKIN_KEY) || "evergreen";
  } catch { /* 同上 */ }
  if (saved !== "light" && saved !== "dark") saved = "system";
  // index.html 内联脚本已防闪；此处对齐控件态并持久化合法值
  if (saved === "system") delete document.documentElement.dataset.theme;
  applyThemeChoice(saved);
  applySkinChoice(skin);
  document.querySelector(".side-foot").addEventListener("click", (event) => {
    const themeBtn = event.target.closest("[data-theme-choice]");
    if (themeBtn) applyThemeChoice(themeBtn.dataset.themeChoice);
    const skinBtn = event.target.closest("[data-skin-choice]");
    if (skinBtn) applySkinChoice(skinBtn.dataset.skinChoice);
  });
}

/* ---------- 壳：品牌区 / 导航 / 页脚 ---------- */

const ROUTES = [
  { path: "tokens", label: "设计 Tokens", icon: "palette", view: renderTokensView },
  { path: "components", label: "组件规格", icon: "layers", view: renderComponentsView },
  { path: "patterns", label: "交互模式", icon: "workflow", view: renderPatternsView },
  { path: "platforms", label: "平台 · 手机/TV", icon: "monitor", view: () => import("./views/platforms.mjs").then((m) => m.renderPlatformsView({ fixtures })) },
];

let TOKENS_VERSION = "";

function renderChrome(active) {
  const brand = document.getElementById("brand");
  brand.replaceChildren();
  const tile = document.createElement("span");
  tile.className = "brand-tile";
  tile.append(icon("sparkles", "icon"));
  const name = document.createElement("span");
  name.className = "brand-name";
  name.textContent = "MediaFlow Design";
  const ver = document.createElement("span");
  ver.className = "brand-ver";
  ver.textContent = TOKENS_VERSION ? `v${TOKENS_VERSION}` : "";
  brand.append(tile, name, ver);

  const nav = document.getElementById("side-nav");
  nav.replaceChildren(
    ...ROUTES.map((r) => {
      const a = document.createElement("a");
      a.href = `#/${r.path}`;
      a.className = r.path === active ? "is-active" : "";
      a.setAttribute("aria-current", r.path === active ? "page" : "false");
      a.append(icon(r.icon), (() => {
        const span = document.createElement("span");
        span.className = "nav-label";
        span.textContent = r.label;
        return span;
      })());
      return a;
    })
  );

  const foot = document.getElementById("foot-note");
  foot.textContent = "tokens.json 唯一源 · 双主题 WCAG 2.2 AA · 零依赖静态站";
}

/* ---------- hash 路由 ---------- */

function currentPath() {
  const raw = location.hash.replace(/^#\/?/, "").split("?")[0];
  return ROUTES.some((r) => r.path === raw) ? raw : "tokens";
}

async function render() {
  const path = currentPath();
  const tokens = await fetchTokens();
  TOKENS_VERSION = tokens.version;
  renderChrome(path);
  const route = ROUTES.find((r) => r.path === path);
  const main = document.getElementById("main");
  main.replaceChildren();
  main.append(await route.view({ tokens, fixtures }));
  main.focus({ preventScroll: true });
  window.scrollTo(0, 0);
}

// 静态壳内的图标占位（主题控件）
for (const slot of document.querySelectorAll("[data-icon]")) {
  slot.replaceChildren(icon(slot.dataset.icon, "icon-sm"));
}

initTheme();
window.addEventListener("hashchange", render);
render();
