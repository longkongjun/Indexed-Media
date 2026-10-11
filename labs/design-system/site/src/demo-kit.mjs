// 共享 demo 构建工具（零依赖）：Lucide 风格线性 SVG 图标 + hero/specimen/badge/button 构建器。
// 图标策略（设计语言 §6.5 的目标态）：SVG 线性图标为主，Unicode 形状符为无图标环境降级通道——
// badge()/stateIcon() 接受字形字符或图标名，字形字符经 GLYPH_ICON 映射为 SVG，未映射则原样渲染。

/* ---------- 图标集（24 viewBox / stroke 2 / round caps，Lucide 同构路径） ---------- */

const PATHS = {
  "check": '<path d="M20 6 9 17l-5-5"/>',
  "circle-check": '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
  "circle-half": '<circle cx="12" cy="12" r="10"/><path d="M12 2a10 10 0 0 1 0 20z" fill="currentColor" stroke="none"/>',
  "circle-dashed": '<circle cx="12" cy="12" r="10" stroke-dasharray="4 3"/><path d="m9 12 2 2 4-4"/>',
  "triangle-alert": '<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 20h16a2 2 0 0 0 1.73-2Z"/><path d="M12 9v4"/><path d="M12 17h.01"/>',
  "octagon-alert": '<path d="M7.86 2h8.28L22 7.86v8.28L16.14 22H7.86L2 16.14V7.86L7.86 2Z"/><path d="M12 8v4"/><path d="M12 16h.01"/>',
  "circle-alert": '<circle cx="12" cy="12" r="10"/><path d="M12 8v4"/><path d="M12 16h.01"/>',
  "clock": '<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>',
  "loader": '<path d="M21 12a9 9 0 1 1-6.22-8.56"/>',
  "circle-slash": '<circle cx="12" cy="12" r="10"/><path d="m4.9 4.9 14.2 14.2"/>',
  "circle": '<circle cx="12" cy="12" r="6" fill="currentColor" stroke="none"/>',
  "circle-ring": '<circle cx="12" cy="12" r="7"/>',
  "scale": '<path d="m16 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/><path d="m2 16 3-8 3 8c-.87.65-1.92 1-3 1s-2.13-.35-3-1Z"/><path d="M7 21h10"/><path d="M12 3v18"/><path d="M3 7h2c2 0 5-1 7-2 2 1 5 2 7 2h2"/>',
  "zap-off": '<path d="M13.52 21.66a.6.6 0 0 1-1.05.47L8.34 16.9a.6.6 0 0 1 .44-1H12l2.22-8.5a.6.6 0 0 1 1.05-.24l4.13 5.24a.6.6 0 0 1-.44 1H15l-2.22 8.5Z"/><path d="M2 2l20 20"/>',
  "inbox": '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z"/>',
  "arrow-right": '<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>',
  "pencil": '<path d="M21.17 6.83a2.83 2.83 0 0 0-4-4L3 16v5h5Z"/><path d="m15 5 4 4"/>',
  "flag": '<path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1Z"/><path d="M4 22v-7"/>',
  "file-text": '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v5h6"/><path d="M10 9H8"/><path d="M16 13H8"/><path d="M16 17H8"/>',
  "image": '<rect width="18" height="18" x="3" y="3" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21"/>',
  "sun": '<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>',
  "moon": '<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>',
  "monitor": '<rect width="20" height="14" x="2" y="3" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/>',
  "palette": '<circle cx="13.5" cy="6.5" r=".5" fill="currentColor"/><circle cx="17.5" cy="10.5" r=".5" fill="currentColor"/><circle cx="8.5" cy="7.5" r=".5" fill="currentColor"/><circle cx="6.5" cy="12.5" r=".5" fill="currentColor"/><path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.93 0 1.68-.75 1.68-1.68 0-.44-.16-.84-.44-1.14-.28-.3-.44-.7-.44-1.14 0-.93.75-1.68 1.68-1.68H16c3.31 0 6-2.69 6-6 0-4.96-4.5-8.36-10-8.36Z"/>',
  "layers": '<path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z"/><path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65"/><path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65"/>',
  "workflow": '<rect width="8" height="8" x="3" y="3" rx="2"/><path d="M7 11v4a2 2 0 0 0 2 2h4"/><rect width="8" height="8" x="13" y="13" rx="2"/>',
  "copy": '<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>',
  "user": '<path d="M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
  "sparkles": '<path d="M9.94 14.06 3 21"/><path d="M12.5 2.5 14 7l4.5 1.5L14 10l-1.5 4.5L11 10 6.5 8.5 11 7Z"/><path d="M19 15l.75 2.25L22 18l-2.25.75L19 21l-.75-2.25L16 18l2.25-.75Z"/>',
  "folder": '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>',
  "bell": '<path d="M10.27 21a2 2 0 0 0 3.46 0"/><path d="M3.08 14.86c-.6-.51-.16-1.7.62-1.7H6v-3a6 6 0 0 1 12 0v3h2.3c.78 0 1.22 1.19.62 1.7l-2.72 2.3A2 2 0 0 1 17 19H7a2 2 0 0 1-1.2-.84Z"/>',
  "search": '<circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/>',
  "chevron-left": '<path d="m15 18-6-6 6-6"/>',
  "chevron-right": '<path d="m9 18 6-6-6-6"/>',
  "x": '<path d="M18 6 6 18"/><path d="m6 6 12 12"/>',
};

// 字形字符 → 图标名（降级映射：规格文档中的形状符在此转为 SVG）
const GLYPH_ICON = {
  "◆": "circle-check", "✓": "check",
  "◐": "circle-half",
  "▲": "triangle-alert", "{!}": "octagon-alert",
  "◌": "loader", "○": "circle-ring", "●": "circle",
  "⚖": "scale", "⌁": "zap-off", "▢": "inbox",
  "→": "arrow-right", "✎": "pencil", "⚑": "flag", "▤": "file-text",
  "▣": "image", "□": "image", "◈": "palette", "⇄": "workflow",
};

export function icon(name, cls = "icon") {
  const path = PATHS[name] ?? PATHS[GLYPH_ICON[name] ?? ""];
  if (!path) return document.createTextNode(name); // 未映射：退回字符（降级通道）
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("class", cls);
  svg.innerHTML = path;
  return svg;
}

/* ---------- hero（eyebrow + display 标题 + 描述） ---------- */

export function hero(eyebrowText, titleHtml, descText) {
  const wrap = document.createElement("header");
  wrap.className = "page-hero";
  const eyebrow = document.createElement("p");
  eyebrow.className = "eyebrow";
  eyebrow.textContent = eyebrowText;
  const h1 = document.createElement("h1");
  h1.className = "page-title";
  h1.innerHTML = titleHtml; // titleHtml 受控调用方拼接，em 包裹关键词
  const desc = document.createElement("p");
  desc.className = "page-desc";
  desc.textContent = descText;
  wrap.append(eyebrow, h1, desc);
  return wrap;
}

/* ---------- specimen（组件标本框；tag 右上角 mono 标签） ---------- */

export function specimen({ tag = null, dots = false } = {}, ...children) {
  const d = document.createElement("div");
  d.className = "specimen";
  if (dots) d.dataset.dots = "";
  if (tag) {
    const t = document.createElement("span");
    t.className = "specimen-tag";
    t.textContent = tag;
    d.append(t);
  }
  d.append(...children.filter(Boolean));
  return d;
}

/* ---------- 章节与基础构件 ---------- */

export function section(id, title, desc, { tag = null } = {}, ...body) {
  const s = document.createElement("section");
  s.className = "section";
  s.id = id;
  const head = document.createElement("div");
  head.className = "section-head";
  const h2 = document.createElement("h2");
  h2.className = "section-title";
  h2.textContent = title;
  head.append(h2);
  if (tag) {
    const t = document.createElement("span");
    t.className = "section-tag";
    t.textContent = tag;
    head.append(t);
  }
  const p = document.createElement("p");
  p.className = "section-desc";
  p.textContent = desc;
  s.append(head, p, ...body);
  return s;
}

// panel = 无标签 specimen（兼容既有视图调用）
export function panel(...children) {
  return specimen({}, ...children);
}

export function note(text) {
  const p = document.createElement("p");
  p.className = "demo-note";
  p.textContent = text;
  return p;
}

export function badge(glyphOrName, label, tone, compact = false) {
  const span = document.createElement("span");
  span.className = `badge badge-${tone}${compact ? " badge-compact" : ""}`;
  span.append(icon(glyphOrName), document.createTextNode(label));
  return span;
}

export function button(text, variant = "primary", extra = "") {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `btn btn-${variant}${extra ? " " + extra : ""}`;
  b.innerHTML = text;
  return b;
}
