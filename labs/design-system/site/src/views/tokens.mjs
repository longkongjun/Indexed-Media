// Tokens 展示页：数据驱动渲染全部语义 token——双值 swatch（点击复制 + 对白/对黑对比度徽章）
// + 字体/间距/圆角/阴影/常数可视化。对比度计算与 scripts/check-contrast.mjs 同公式。

import { section, hero } from "../demo-kit.mjs";

function luminance([r, g, b]) {
  const lin = (v8) => {
    const c = v8 / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function parseHex(input) {
  const m = input.match(/^#([0-9a-f]{6})$/i);
  if (!m) return null;
  const h = m[1];
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

function ratioTo(fg, bgRgb) {
  const l1 = luminance(fg);
  const l2 = luminance(bgRgb);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

const WHITE = [255, 255, 255];
const BLACK = [0, 0, 0];

/** 实时读取当前 皮肤×主题 下某 token 的计算值（皮肤体系的可视化基础）。 */
function liveValue(name) {
  return getComputedStyle(document.documentElement)
    .getPropertyValue(`--mfd-${name}`)
    .trim();
}

function chip(name) {
  const div = document.createElement("div");
  div.className = "chip";
  div.dataset.token = name;
  paintChip(div);
  return div;
}

function paintChip(div) {
  const value = liveValue(div.dataset.token);
  div.style.background = value;
  div.textContent = value || "—";
  const rgb = parseHex(value);
  if (rgb) {
    div.style.color = ratioTo(rgb, BLACK) >= ratioTo(rgb, WHITE) ? "#000000" : "#ffffff";
    const r = div.querySelector(".ratio") ?? div.appendChild(Object.assign(document.createElement("span"), { className: "ratio" }));
    r.textContent = `白 ${ratioTo(rgb, WHITE).toFixed(1)}·黑 ${ratioTo(rgb, BLACK).toFixed(1)}`;
  }
}

function swatchGrid(entries) {
  const grid = document.createElement("div");
  grid.className = "swatch-grid";
  for (const { name, desc, light, dark } of entries) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "swatch";
    card.setAttribute("aria-label", `复制 ${name} 的 token 值`);
    const chips = document.createElement("div");
    chips.className = "chips";
    chips.append(chip(name));
    const body = document.createElement("span");
    body.className = "swatch-body";
    const nm = document.createElement("span");
    nm.className = "swatch-name";
    nm.textContent = name;
    const lbl = document.createElement("span");
    lbl.className = "swatch-label";
    lbl.textContent = desc;
    body.append(nm, lbl);
    card.append(chips, body);
    card.addEventListener("click", () => {
      const text = `--mfd-${name}（值随 皮肤×主题 组合，见 tokens/skins/*.json）`;
      navigator.clipboard?.writeText(`--mfd-${name}`).catch(() => {});
      const overlay = document.createElement("span");
      overlay.className = "copied";
      overlay.textContent = "已复制 token";
      card.append(overlay);
      setTimeout(() => overlay.remove(), 900);
    });
    grid.append(card);
  }
  return grid;
}

function collectColorTokens(tokens, group) {
  const entries = [];
  const walk = (node, prefix) => {
    for (const [key, value] of Object.entries(node)) {
      if (key.startsWith("_")) continue;
      if (typeof value === "object" && !("light" in value)) walk(value, `${prefix}${key}-`);
      else entries.push({ name: `color-${prefix}${key}`, desc: value.desc ?? "", light: value.light, dark: value.dark });
    }
  };
  walk(tokens.semantic.color[group], `${group}-`);
  return entries;
}

let liveRefreshBound = false;
function bindLiveRefresh(frag) {
  if (liveRefreshBound) return;
  liveRefreshBound = true;
  const refresh = () => {
    for (const chipEl of document.querySelectorAll(".swatch .chip")) paintChip(chipEl);
    const ctx = document.querySelector("[data-live-context]");
    if (ctx) ctx.textContent = currentContextLabel();
  };
  document.documentElement.addEventListener("mfd-skin-change", refresh);
  document.documentElement.addEventListener("mfd-theme-change", refresh);
}

function currentContextLabel() {
  const skin = document.documentElement.dataset.skin || "evergreen";
  const theme = document.documentElement.dataset.theme || (matchMedia("(prefers-color-scheme: dark)").matches ? "dark(系统)" : "light(系统)");
  const LABELS = { evergreen: "常青", aurora: "极光", sepia: "胶片" };
  return `当前组合：皮肤 ${LABELS[skin] ?? skin} × 主题 ${theme}（色卡为实时计算值）`;
}

export async function renderTokensView({ tokens }) {
  const frag = document.createDocumentFragment();

  frag.append(hero(
    "Design Tokens",
    '常青色板，<em>双主题</em>语义',
    `v${tokens.version} · 皮肤（常青/极光/胶片）× 主题（浅/暗）二维体系 · 色卡显示当前组合的实时值并随侧栏切换刷新 · 点击复制 token 名 · 色角为对白/对黑对比度（WCAG 公式）· 每皮肤 62 必检对双主题全过 AA。`,
  ));

  const ctx = document.createElement("p");
  ctx.className = "eyebrow";
  ctx.dataset.liveContext = "";
  ctx.textContent = currentContextLabel();
  frag.append(ctx);
  bindLiveRefresh(frag);

  for (const [group, title, blurb, tag] of [
    ["bg", "背景色", "容器层级：canvas → surface → raised/sunken；inverse 为侧栏深底", "surface"],
    ["text", "文字色", "主/次/弱 + 反色区/操作底文字 + 链接", "content"],
    ["border", "边框色", "strong/focus 为交互识别边界（≥3:1）；default/subtle 为装饰级", "line"],
    ["action", "操作色", "主操作暗色为亮绿底+深字（D-0005）；gradient 为可选品牌瞬间", "action"],
    ["state", "状态色", "成功/待处理/失败/进行中四语义 × 前景/底/描边", "status"],
    ["media", "媒体色", "海报角标深底恒定；图片本体零主题处理", "media"],
    ["chart", "图表色", "五系列色 + 网格/轴（ECharts 按需消费）", "chart"],
  ]) {
    frag.append(section(`token-${group}`, title, blurb, { tag }, swatchGrid(collectColorTokens(tokens, group))));
  }

  // 字体
  const typeDemo = document.createElement("div");
  typeDemo.className = "specimen type-demo";
  for (const [key, item] of Object.entries(tokens.semantic.font.size)) {
    if (key.startsWith("_")) continue;
    const p = document.createElement("p");
    p.style.fontSize = item.value;
    p.style.lineHeight = "var(--mfd-font-line-height-tight)";
    const tagEl = document.createElement("span");
    tagEl.className = "tag";
    tagEl.textContent = `font-size-${key} · ${item.value} · ${item.desc}`;
    const sample = document.createElement("span");
    sample.textContent = "媒体库的安静管理者 MediaFlow";
    p.append(tagEl, sample);
    typeDemo.append(p);
  }
  const mono = document.createElement("p");
  const monoTag = document.createElement("span");
  monoTag.className = "tag";
  monoTag.textContent = "font-family-mono · 路径与技术标识";
  const monoSample = document.createElement("span");
  monoSample.style.fontFamily = "var(--mfd-font-family-mono)";
  monoSample.textContent = "媒体库/电影/沙丘 2 (2024)/dune2.mkv";
  mono.append(monoTag, monoSample);
  typeDemo.append(mono);
  frag.append(section("token-type", "字体阶梯", "Inter + 系统回退；管理表格 14px 档；等宽用于路径/规则/ID；display 档用于展示级标题", { tag: "type" }, typeDemo));

  // 间距
  const spaceScale = document.createElement("div");
  spaceScale.className = "specimen space-scale";
  for (const [key, item] of Object.entries(tokens.semantic.space)) {
    if (key.startsWith("_")) continue;
    const row = document.createElement("div");
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = `space-${key} · ${item.value} · ${item.desc}`;
    const bar = document.createElement("div");
    bar.className = "bar";
    bar.style.width = item.value;
    row.append(tag, bar);
    spaceScale.append(row);
  }
  frag.append(section("token-space", "间距（4pt 基）", "space-1 至 space-16", { tag: "space" }, spaceScale));

  // 圆角
  const radiusDemo = document.createElement("div");
  radiusDemo.className = "specimen radius-demo";
  for (const [key, item] of Object.entries(tokens.semantic.radius)) {
    if (key.startsWith("_")) continue;
    const box = document.createElement("div");
    box.style.borderRadius = item.value;
    box.title = `radius-${key} · ${item.value}`;
    radiusDemo.append(box);
  }
  frag.append(section("token-radius", "圆角", "xs → 2xl → full 七档", { tag: "radius" }, radiusDemo));

  // 阴影
  const shadowDemo = document.createElement("div");
  shadowDemo.className = "demo-grid shadow-demo";
  for (const key of ["1", "2", "3"]) {
    const box = document.createElement("div");
    box.className = "specimen cell";
    box.style.boxShadow = `var(--mfd-shadow-${key})`;
    box.textContent = `shadow-${key}`;
    shadowDemo.append(box);
  }
  frag.append(section("token-shadow", "海拔（分层阴影）", "v1.1 起为双层叠加阴影；暗色加深并由描边补偿层级", { tag: "elevation" }, shadowDemo));

  // 常数
  const misc = document.createElement("div");
  misc.className = "specimen";
  misc.innerHTML = `
    <p><strong>断点</strong>：sm ${tokens.semantic.breakpoint.sm.value}（移动/桌面）· md ${tokens.semantic.breakpoint.md.value}（侧栏图标化）· lg ${tokens.semantic.breakpoint.lg.value}（宽屏上限）</p>
    <p><strong>动效</strong>：fast ${tokens.semantic.motion.duration.fast.value} 微交互 · base ${tokens.semantic.motion.duration.base.value} 容器 · slow ${tokens.semantic.motion.duration.slow.value} 主题过渡；easing standard / in-out；respect prefers-reduced-motion</p>
    <p><strong>图标</strong>：sm ${tokens.semantic.icon.size.sm.value} / md ${tokens.semantic.icon.size.md.value} / lg ${tokens.semantic.icon.size.lg.value}；触达下限 ${tokens.semantic.icon["size-touch-min"].value}；本站图标为 Lucide 风格线性 SVG（字形符为降级通道）</p>
    <p><strong>描边</strong>：hair ${tokens.semantic.stroke.hair.value} · focus-ring ${tokens.semantic.stroke["focus-ring"].value} · strong ${tokens.semantic.stroke.strong.value}</p>`;
  frag.append(section("token-misc", "断点 · 动效 · 图标 · 描边", "布局与运动常数", { tag: "constants" }, misc));

  return frag;
}
