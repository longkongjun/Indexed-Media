// 风格指南站入口：主题控制器（theme-switching 模式 §1-2）+ hash 路由。
// 零依赖：原生 ES Modules；数据 fetch 本地 assets/tokens.json 与 fixtures。

import { renderTokensView } from "./views/tokens.mjs";
import { renderComponentsView } from "./views/components.mjs";
import { renderPatternsView } from "./views/patterns.mjs";
import { fetchTokens, fixtures } from "./fixtures.mjs";

/* ---------- 主题三态 ---------- */

const THEME_KEY = "mfd-theme";

function applyThemeChoice(choice) {
  const root = document.documentElement;
  if (choice === "light" || choice === "dark") root.dataset.theme = choice;
  else delete root.dataset.theme; // 跟随系统：交给 prefers-color-scheme
  try { localStorage.setItem(THEME_KEY, choice); } catch { /* 隐私模式等：仅本次会话生效 */ }
  for (const btn of document.querySelectorAll("[data-theme-choice]")) {
    btn.setAttribute("aria-checked", String(btn.dataset.themeChoice === choice));
  }
}

function initTheme() {
  let saved = "system";
  try { saved = localStorage.getItem(THEME_KEY) || "system"; } catch { /* 同上 */ }
  if (saved !== "light" && saved !== "dark") saved = "system";
  // index.html 内联脚本已防闪设了 data-theme；此处对齐控件态并持久化合法值
  if (saved === "system") delete document.documentElement.dataset.theme;
  applyThemeChoice(saved);
  document.querySelector(".theme-box").addEventListener("click", (event) => {
    const btn = event.target.closest("[data-theme-choice]");
    if (btn) applyThemeChoice(btn.dataset.themeChoice);
  });
}

/* ---------- hash 路由 ---------- */

const ROUTES = [
  { path: "tokens", label: "设计 Tokens", view: renderTokensView },
  { path: "components", label: "组件规格", view: renderComponentsView },
  { path: "patterns", label: "交互模式", view: renderPatternsView },
];

function currentPath() {
  const raw = location.hash.replace(/^#\/?/, "").split("?")[0];
  return ROUTES.some((r) => r.path === raw) ? raw : "tokens";
}

function renderNav(active) {
  const nav = document.getElementById("side-nav");
  nav.replaceChildren(
    ...ROUTES.map((r) => {
      const a = document.createElement("a");
      a.href = `#/${r.path}`;
      a.className = r.path === active ? "is-active" : "";
      a.setAttribute("aria-current", r.path === active ? "page" : "false");
      const glyph = document.createElement("span");
      glyph.textContent = { tokens: "◈", components: "▤", patterns: "⇄" }[r.path];
      glyph.setAttribute("aria-hidden", "true");
      const span = document.createElement("span");
      span.textContent = r.label;
      a.append(glyph, span);
      return a;
    })
  );
}

async function render() {
  const path = currentPath();
  renderNav(path);
  const route = ROUTES.find((r) => r.path === path);
  const main = document.getElementById("main");
  main.replaceChildren();
  main.append(await route.view({ tokens: await fetchTokens(), fixtures }));
  main.focus({ preventScroll: true });
  window.scrollTo(0, 0);
}

initTheme();
window.addEventListener("hashchange", render);
render();
