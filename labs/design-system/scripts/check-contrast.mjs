#!/usr/bin/env node
// 双主题 × 全皮肤对比度验证（WCAG 2.2 AA）：evergreen 基线 + tokens/skins/*.json 逐皮肤 × {light, dark}。
// text ≥ 4.5:1，non-text ≥ 3:1；advisory 仅报告。失败退出码 1。零依赖。

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const doc = JSON.parse(readFileSync(join(here, "..", "tokens", "tokens.json"), "utf8"));
const skinsDir = join(here, "..", "tokens", "skins");

const isColor = (v) => v && typeof v === "object" && "light" in v;

function flatten(node, prefix, out) {
  for (const [key, value] of Object.entries(node)) {
    if (key.startsWith("_") || key === "style") continue;
    const name = prefix ? `${prefix}-${key}` : key;
    if (value && typeof value === "object" && !isColor(value) && !("value" in value)) {
      flatten(value, name, out);
    } else {
      out[name] = value;
    }
  }
  return out;
}

function parseColor(input) {
  const hex = input.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (hex) {
    const h = hex[1].length === 3 ? [...hex[1]].map((c) => c + c).join("") : hex[1];
    return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16), 1];
  }
  const rgba = input.match(/^rgba?\(\s*([\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)(?:[\s/]+([\d.]+))?\s*\)$/i);
  if (rgba) return [+rgba[1], +rgba[2], +rgba[3], rgba[4] === undefined ? 1 : +rgba[4]];
  return null;
}

function luminance([r, g, b]) {
  const lin = (v8) => {
    const c = v8 / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function blend(fg, bg) {
  const a = fg[3];
  if (a >= 1) return fg;
  return [0, 1, 2].map((i) => fg[i] * a + bg[i] * (1 - a)).concat(1);
}

function ratioOf(fgRaw, bgRaw) {
  const fg = parseColor(fgRaw);
  const bg = parseColor(bgRaw);
  if (!fg || !bg) return null;
  const l1 = luminance(blend(fg, bg));
  const l2 = luminance(bg);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

/** 构建 (skin, theme) → 扁平 token 视图（皮肤深合并覆盖基线）。 */
function buildViews() {
  const baseline = flatten(doc.semantic, "", Object.create(null));
  const views = new Map([["evergreen", baseline]]);
  for (const file of readdirSync(skinsDir).filter((f) => f.endsWith(".json")).sort()) {
    const skin = JSON.parse(readFileSync(join(skinsDir, file), "utf8"));
    const flatSkin = flatten(skin.color ?? {}, "color", Object.create(null));
    views.set(skin.name ?? file.replace(".json", ""), { ...baseline, ...flatSkin });
  }
  return views;
}

function themeValue(view, name, theme) {
  const token = view[name];
  if (!token) return undefined;
  if (typeof token === "object" && "light" in token) return token[theme];
  return token.value;
}

const views = buildViews();
const results = [];
let failures = 0;
let skipped = 0;
let total = 0;

for (const [group, min, enforced] of [["text", 4.5, true], ["non-text", 3, true], ["advisory", 3, false]]) {
  for (const [fgName, bgName] of doc.contrast_pairs[group] ?? []) {
    for (const [skin, view] of views) {
      for (const theme of ["light", "dark"]) {
        const fg = themeValue(view, fgName, theme);
        const bg = themeValue(view, bgName, theme);
        if (fg === undefined || bg === undefined) {
          results.push({ group, fgName, bgName, skin, theme, ratio: null, ok: false, note: "token 缺失" });
          if (enforced) failures += 1;
          continue;
        }
        const ratio = ratioOf(fg, bg);
        if (ratio === null) {
          skipped += 1;
          continue;
        }
        const ok = ratio >= min;
        if (!ok && enforced) failures += 1;
        results.push({ group, fgName, bgName, skin, theme, ratio, ok, min, enforced });
      }
    }
  }
}

console.log(`MediaFlow Design tokens v${doc.version} — 双主题 × 全皮肤对比度验证（WCAG 2.2 AA）`);
console.log(`皮肤：${[...views.keys()].join(" · ")}（每皮肤 × light/dark）`);
console.log("=".repeat(104));
const bySkin = new Map();
for (const r of results) {
  if (r.enforced === false && r.ok) continue; // advisory 通过的不打印
  const key = r.skin;
  if (!bySkin.has(key)) bySkin.set(key, { pass: 0, fail: 0, advi: 0 });
  const bucket = bySkin.get(key);
  if (r.ok) bucket.pass += 1;
  else if (r.enforced === false) bucket.advi += 1;
  else {
    bucket.fail += 1;
    const mark = r.enforced === false ? "ADVI" : "FAIL";
    console.log(`${mark}  ${String(r.min)}  ${(r.ratio ?? 0).toFixed(2).padStart(5)}  ${r.theme.padEnd(5)} ${r.fgName} on ${r.bgName}${r.note ? ` — ${r.note}` : ""}${r.enforced === false ? "（advisory，不判定）" : ""}`);
  }
}
console.log("=".repeat(104));
for (const [skin, b] of bySkin) console.log(`${skin}: ${b.pass}/${b.pass + b.fail} 通过，${b.fail} 失败${b.advi ? `，${b.advi} advisory（装饰性）` : ""}`);
console.log(`合计 ${results.filter((r) => r.ok).length}/${results.length} 通过，${failures} 失败，${skipped} 跳过（渐变/复杂值不参与）。`);
process.exit(failures > 0 ? 1 : 0);
