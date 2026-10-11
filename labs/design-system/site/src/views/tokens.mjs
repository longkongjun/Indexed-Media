// Tokens 展示页：数据驱动渲染全部语义 token（浅/暗双值并排）+ 尺寸类可视化。

const cssVar = (name) => `var(--mfd-${name})`;

function section(id, title, desc, specLink, ...body) {
  const section = document.createElement("section");
  section.className = "section";
  section.id = id;
  const h2 = document.createElement("h2");
  h2.className = "section-title";
  h2.textContent = title;
  section.append(h2);
  if (desc) {
    const p = document.createElement("p");
    p.className = "section-desc";
    p.textContent = desc;
    section.append(p);
  }
  if (specLink) {
    const a = document.createElement("a");
    a.className = "spec-link";
    a.href = specLink;
    a.textContent = "规格文档 →";
    section.append(a);
  }
  section.append(...body);
  return section;
}

function swatchGrid(entries) {
  const grid = document.createElement("div");
  grid.className = "swatch-grid";
  for (const { name, token, desc, light, dark } of entries) {
    const card = document.createElement("div");
    card.className = "swatch";
    card.innerHTML = `
      <div class="chip light-value" style="--swatch:${light}" title="浅色值">${light}</div>
      <div class="chip dark-value" style="--swatch-dark:${dark}" title="暗色值">${dark}</div>
      <span class="swatch-name">${name}</span>
      <span class="swatch-label">${desc}</span>`;
    // name/desc 为受控文本，直接 innerHTML 无注入面；token 变量供复制检查
    card.dataset.token = token;
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
      else entries.push({ name: `${prefix}${key}`, token: `--mfd-color-${prefix}${key}`, desc: value.desc ?? "", light: value.light, dark: value.dark });
    }
  };
  walk(tokens.semantic.color[group], "");
  return entries;
}

export async function renderTokensView({ tokens }) {
  const frag = document.createDocumentFragment();

  const h1 = document.createElement("h1");
  h1.className = "page-title";
  h1.textContent = "设计 Tokens";
  const desc = document.createElement("p");
  desc.className = "page-desc";
  desc.textContent = `MediaFlow Design v${tokens.version} · 唯一源 tokens/tokens.json · 每个语义 token 双主题双值 · 对比度 62 必检对双主题全过（WCAG 2.2 AA）`;
  frag.append(h1, desc);

  for (const [group, title, blurb] of [
    ["bg", "背景", "容器层级：canvas → surface → raised/sunken；inverse 为侧栏深底"],
    ["text", "文字", "主/次/弱 + 反色区/操作底文字 + 链接"],
    ["border", "边框", "strong/focus 为交互识别边界（≥3:1）；default/subtle 为装饰级"],
    ["action", "操作", "主操作暗色为亮绿底+深字（D-0005）"],
    ["state", "状态", "成功/待处理/失败/进行中四语义 × 前景/底/描边"],
    ["media", "媒体", "海报角标深底恒定；图片本体零主题处理（dark-media-principles）"],
    ["chart", "图表", "五系列色 + 网格/轴（ECharts 按需消费）"],
  ]) {
    frag.append(section(`token-${group}`, `${title}色`, blurb, null, swatchGrid(collectColorTokens(tokens, group))));
  }

  // 字体
  const typeDemo = document.createElement("div");
  typeDemo.className = "type-demo panel";
  const scale = tokens.semantic.font.size;
  for (const [key, item] of Object.entries(scale)) {
    if (key.startsWith("_")) continue;
    const p = document.createElement("p");
    p.style.fontSize = item.value;
    p.style.lineHeight = "var(--mfd-font-line-height-tight)";
    p.innerHTML = `<span class="tag">font-size-${key} · ${item.value} · ${item.desc}</span>媒体库的安静管理者 MediaFlow`;
    typeDemo.append(p);
  }
  const mono = document.createElement("p");
  mono.innerHTML = `<span class="tag">font-family-mono · 路径与技术标识</span><span style="font-family:var(--mfd-font-family-mono)">媒体库/电影/沙丘 2 (2024)/dune2.part.mkv</span>`;
  typeDemo.append(mono);
  frag.append(section("token-type", "字体阶梯", "Inter + 系统回退；管理表格 14px 档；等宽用于路径/规则/ID", null, typeDemo));

  // 间距
  const spaceScale = document.createElement("div");
  spaceScale.className = "space-scale panel";
  for (const [key, item] of Object.entries(tokens.semantic.space)) {
    if (key.startsWith("_")) continue;
    const row = document.createElement("div");
    const bar = document.createElement("div");
    bar.style.width = item.value;
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = `space-${key} · ${item.value} · ${item.desc}`;
    row.append(tag, bar);
    spaceScale.append(row);
  }
  frag.append(section("token-space", "间距（4pt 基）", "space-1 至 space-16", null, spaceScale));

  // 圆角
  const radiusDemo = document.createElement("div");
  radiusDemo.className = "radius-demo panel";
  for (const [key, item] of Object.entries(tokens.semantic.radius)) {
    if (key.startsWith("_")) continue;
    const box = document.createElement("div");
    box.style.borderRadius = item.value;
    box.title = `radius-${key} · ${item.value}`;
    radiusDemo.append(box);
  }
  frag.append(section("token-radius", "圆角", "xs → full 六档", null, radiusDemo));

  // 阴影
  const shadowDemo = document.createElement("div");
  shadowDemo.className = "demo-grid shadow-demo";
  for (const key of ["1", "2", "3"]) {
    const box = document.createElement("div");
    box.className = "panel";
    box.style.boxShadow = cssVar(`shadow-${key}`);
    box.textContent = `shadow-${key}`;
    shadowDemo.append(box);
  }
  frag.append(section("token-shadow", "海拔（阴影）", "暗色下阴影加深并由描边补偿层级", null, shadowDemo));

  // 断点与动效（文本表）
  const misc = document.createElement("div");
  misc.className = "panel";
  misc.innerHTML = `
    <p><strong>断点</strong>：sm ${tokens.semantic.breakpoint.sm.value}（移动/桌面）· md ${tokens.semantic.breakpoint.md.value}（侧栏图标化）· lg ${tokens.semantic.breakpoint.lg.value}（宽屏上限）</p>
    <p><strong>动效</strong>：fast ${tokens.semantic.motion.duration.fast.value} 微交互 · base ${tokens.semantic.motion.duration.base.value} 容器 · slow ${tokens.semantic.motion.duration.slow.value} 主题过渡；easing standard / in-out；respect prefers-reduced-motion</p>
    <p><strong>图标</strong>：sm ${tokens.semantic.icon.size.sm.value} / md ${tokens.semantic.icon.size.md.value} / lg ${tokens.semantic.icon.size.lg.value}；触达下限 ${tokens.semantic.icon["size-touch-min"].value}</p>
    <p><strong>描边</strong>：hair ${tokens.semantic.stroke.hair.value} · focus-ring ${tokens.semantic.stroke["focus-ring"].value} · strong ${tokens.semantic.stroke.strong.value}</p>`;
  frag.append(section("token-misc", "断点 · 动效 · 图标 · 描边", "布局与运动常数", null, misc));

  return frag;
}
