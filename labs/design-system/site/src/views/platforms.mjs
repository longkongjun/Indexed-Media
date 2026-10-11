// 平台视图：手机（compact）与 TV（10-foot UI）的真实渲染演示 + window size class 对照。
// 手机框复用组件 token 类（媒体墙 2 列 / 列表卡 / 六态一例 / 底部导航）；
// TV 框演示焦点导航（方向键在卡片间移动，Enter 语义），全部键盘可达。

import { section, hero, icon, badge, note, panel } from "../demo-kit.mjs";

const NAV = [
  { icon: "circle-ring", label: "概览", active: false },
  { icon: "layers", label: "任务", active: true },
  { icon: "image", label: "媒体", active: false },
  { icon: "menu-x", label: "更多", active: false },
];

function phoneDevice({ fixtures }) {
  const device = document.createElement("div");
  device.className = "device";
  const label = document.createElement("div");
  label.className = "device-label";
  label.append(icon("user"), document.createTextNode("手机 · compact（< 48rem / 600dp）"));
  const screen = document.createElement("div");
  screen.className = "phone-screen";

  const bar = document.createElement("div");
  bar.className = "phone-statusbar";
  bar.innerHTML = "<span>9:41</span><span>MediaFlow</span>";

  const body = document.createElement("div");
  body.className = "phone-body";

  // 媒体墙（2 列）
  const wall = document.createElement("div");
  wall.className = "wall";
  wall.style.gridTemplateColumns = "1fr 1fr";
  for (const m of fixtures.media.slice(0, 2)) {
    const card = document.createElement("a");
    card.className = "media-card";
    card.href = "#/platforms";
    card.setAttribute("aria-label", `《${m.title}》详情`);
    card.innerHTML = `<div class="poster">${m.registered ? '<span class="art"><span class="art-grad"></span><span class="label">海报</span></span>' : `<span class="ph" aria-hidden="true">${""}</span>`}<span class="corner">${m.kind}</span></div><div class="cap">${m.title}<span class="meta">${m.year || ""}</span></div>`;
    const ph = card.querySelector(".ph");
    if (ph) ph.append(icon("image", "icon-lg"));
    wall.append(card);
  }

  // 列表卡（任务）
  const list = document.createElement("ul");
  list.className = "row-list";
  list.style.background = "var(--mfd-color-bg-surface)";
  list.style.border = "var(--mfd-stroke-hair) solid var(--mfd-color-border-default)";
  list.style.borderRadius = "var(--mfd-radius-lg)";
  for (const task of fixtures.tasks.slice(1, 3)) {
    const li = document.createElement("li");
    li.className = "clickable";
    li.append(badge(task.statusGlyph, task.statusLabel, task.statusTone));
    const text = document.createElement("span");
    text.style.flex = "1";
    text.innerHTML = `<strong style="font-size:var(--mfd-font-size-sm)">${task.id}</strong><br><span style="font-size:var(--mfd-font-size-xs);color:var(--mfd-color-text-secondary)">${task.summary}</span>`;
    li.append(text, icon("chevron-right"));
    list.append(li);
  }

  // 六态一例（offline）
  const state = document.createElement("div");
  state.className = "state-box";
  state.dataset.state = "offline";
  state.style.padding = "var(--mfd-space-3)";
  const si = document.createElement("span");
  si.className = "state-icon";
  si.setAttribute("aria-hidden", "true");
  si.append(icon("zap-off"));
  const st = document.createElement("span");
  st.className = "state-title";
  st.style.fontSize = "var(--mfd-font-size-base)";
  st.textContent = "无法连接服务";
  const sb = document.createElement("span");
  sb.className = "state-body";
  sb.textContent = "正在显示缓存数据。";
  state.append(si, st, sb);

  body.append(wall, list, state);

  const nav = document.createElement("nav");
  nav.className = "phone-nav";
  nav.setAttribute("aria-label", "底部导航");
  for (const item of NAV) {
    const a = document.createElement("a");
    a.href = "#/platforms";
    if (item.active) a.className = "is-active";
    a.setAttribute("aria-current", item.active ? "page" : "false");
    a.append(icon(item.icon === "menu-x" ? "workflow" : item.icon), document.createTextNode(item.label));
    nav.append(a);
  }

  screen.append(bar, body, nav);
  device.append(label, screen);
  return device;
}

function tvDevice() {
  const device = document.createElement("div");
  device.className = "device";
  const label = document.createElement("div");
  label.className = "device-label";
  label.append(icon("monitor"), document.createTextNode("TV · 10-foot UI（焦点导航）"));
  const screen = document.createElement("div");
  screen.className = "tv-screen";

  const head = document.createElement("div");
  head.style.display = "flex";
  head.style.justifyContent = "space-between";
  head.style.alignItems = "baseline";
  head.innerHTML = `<strong style="font-size:var(--mfd-font-size-lg)">继续观看</strong>`;
  const headMeta = document.createElement("span");
  headMeta.className = "tv-hint";
  headMeta.textContent = "家庭成员 · 浏览与外部播放";
  head.append(headMeta);

  const row = document.createElement("div");
  row.className = "tv-row";
  row.setAttribute("role", "list");
  for (const m of [
    { title: "地球脉动 第三季", meta: "剧集 · 看到 S01E04" },
    { title: "沙丘 2", meta: "电影 · 2024" },
    { title: "家庭聚会 2026", meta: "通用视频" },
  ]) {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "tv-card";
    card.setAttribute("role", "listitem");
    const art = document.createElement("div");
    art.className = "poster";
    art.style.aspectRatio = "16 / 9";
    art.style.flex = "1";
    art.style.position = "relative";
    art.innerHTML = `<span class="art"><span class="art-grad"></span></span>`;
    const title = document.createElement("span");
    title.className = "tv-title";
    title.textContent = m.title;
    const meta = document.createElement("span");
    meta.className = "tv-meta";
    meta.textContent = m.meta;
    card.append(art, title, meta);
    row.append(card);
  }

  const hint = document.createElement("p");
  hint.className = "tv-hint";
  hint.textContent = "遥控器：方向键移动焦点（试试 Tab/←→）· Enter 确认 · 返回 = 上一步；焦点块 ≥ 48dp，管理与审批不出现在 TV";

  screen.append(head, row, hint);
  device.append(label, screen);
  return device;
}

function sizeClassTable() {
  const tbl = document.createElement("table");
  tbl.className = "data-table";
  tbl.innerHTML = `
    <thead><tr><th>size class（M3）</th><th>宽度</th><th>MFD 断点</th><th>导航形态</th><th>内容形态</th></tr></thead>
    <tbody>
      <tr><td>compact</td><td>&lt; 600dp / 48rem</td><td>sm 以下</td><td>底部导航四项</td><td>单列；表格降级列表</td></tr>
      <tr><td>medium</td><td>600–840dp / 48–64rem</td><td>sm–md</td><td>侧栏图标列</td><td>单列为主</td></tr>
      <tr><td>expanded</td><td>&gt; 840dp / 64rem</td><td>md / lg</td><td>固定文字侧栏</td><td>双列可用（lg 上限 90rem）</td></tr>
      <tr><td>TV（10-foot）</td><td>视距 3m</td><td>—</td><td>横向卡片行 + 焦点导航</td><td>只读消费；无管理面</td></tr>
    </tbody>`;
  return tbl;
}

export async function renderPlatformsView({ fixtures }) {
  const frag = document.createDocumentFragment();
  frag.append(hero(
    "Platforms",
    '一套语义，<em>三端</em>形态',
    "同一 token × 皮肤 × 语义体系在手机（compact）与 TV（10-foot UI）的形态演示；皮肤与主题切换对本页同样生效——这正是二维主题体系的验证场景。",
  ));

  const grid = document.createElement("div");
  grid.className = "platform-grid";
  const phone = phoneDevice({ fixtures });
  const phoneCap = document.createElement("p");
  phoneCap.className = "device-caption";
  phoneCap.textContent = "手机：底部导航 / 2 列媒体墙 / 列表卡 / 六态——组件类与桌面同源，仅密度与断点差异";
  const tv = tvDevice();
  const tvCap = document.createElement("p");
  tvCap.className = "device-caption";
  tvCap.textContent = "TV：焦点导航（键盘可达演示）· 大卡片 · 只读消费面；Compose for TV / tvOS Focus Engine 对应";
  const phoneWrap = document.createElement("div");
  phoneWrap.append(phone, phoneCap);
  const tvWrap = document.createElement("div");
  tvWrap.append(tv, tvCap);
  grid.append(phoneWrap, tvWrap);
  frag.append(section("p-devices", "设备形态演示", "真实渲染（非截图）：手机框与 TV 框内的组件由当前皮肤/主题的 CSS 变量驱动", { tag: "live" }, grid));

  frag.append(section(
    "p-sizeclass", "窗口尺寸分级（Material 3 window size classes 对齐）",
    "M3 以 compact/medium/expanded 三级窗口类替代设备类型判断；MFD 断点与之对齐，TV 为独立形态（焦点模型）",
    { tag: "size-class" },
    panel(sizeClassTable()),
    note("映射原则详见 docs/patterns/cross-platform-matrix.md；移动端组件差异见各组件规格 §7"),
  ));

  return frag;
}
