// 共享 demo 构建工具（DOM helper，零依赖）。

export function section(id, title, desc, _slug, ...body) {
  const s = document.createElement("section");
  s.className = "section";
  s.id = id;
  const h2 = document.createElement("h2");
  h2.className = "section-title";
  h2.textContent = title;
  const p = document.createElement("p");
  p.className = "section-desc";
  p.textContent = desc;
  s.append(h2, p, ...body);
  return s;
}

export function panel(...children) {
  const d = document.createElement("div");
  d.className = "panel";
  d.append(...children.filter(Boolean));
  return d;
}

export function note(text) {
  const p = document.createElement("p");
  p.className = "demo-note";
  p.textContent = text;
  return p;
}

export function badge(glyph, label, tone, compact = false) {
  const span = document.createElement("span");
  span.className = `badge badge-${tone}${compact ? " badge-compact" : ""}`;
  span.innerHTML = `<i class="glyph" aria-hidden="true">${glyph}</i>${label}`;
  return span;
}

export function button(text, variant = "primary", extra = "") {
  const b = document.createElement("button");
  b.type = "button";
  b.className = `btn btn-${variant}${extra ? " " + extra : ""}`;
  b.innerHTML = text;
  return b;
}
