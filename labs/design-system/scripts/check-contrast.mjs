#!/usr/bin/env node
// 双主题对比度自动验证：消费 tokens.json 的 contrast_pairs，对 light/dark 各计算一遍 WCAG 比率。
// 基线 WCAG 2.2 AA：text ≥ 4.5:1，non-text ≥ 3:1。失败退出码 1。
// 零依赖：仅用 node:fs / node:path / node:url。

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const doc = JSON.parse(readFileSync(join(here, "..", "tokens", "tokens.json"), "utf8"));

/** 扁平化 semantic 段。 */
function flatten(node, prefix, out) {
  for (const [key, value] of Object.entries(node)) {
    if (key.startsWith("_")) continue;
    const name = prefix ? `${prefix}-${key}` : key;
    if (value && typeof value === "object" && !("light" in value) && !("value" in value)) {
      flatten(value, name, out);
    } else {
      out[name] = value;
    }
  }
  return out;
}

const flat = flatten(doc.semantic, "", Object.create(null));

/** 解析 #rgb/#rrggbb 或 rgba() → [r,g,b,a]（0-255 / 0-1）。 */
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

/** WCAG 相对亮度。 */
function luminance([r, g, b]) {
  const lin = (v8) => {
    const c = v8 / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** 半透明色对给定背景做合成。 */
function blend(fg, bg) {
  const a = fg[3];
  if (a >= 1) return fg;
  return [0, 1, 2].map((i) => fg[i] * a + bg[i] * (1 - a)).concat(1);
}

function ratioOf(fgRaw, bgRaw) {
  const fg = parseColor(fgRaw);
  const bg = parseColor(bgRaw);
  if (!fg || !bg) return null; // 渐变等复杂值不参与计算
  const fgB = blend(fg, bg);
  const l1 = luminance(fgB);
  const l2 = luminance(bg);
  const [hi, lo] = l1 >= l2 ? [l1, l2] : [l2, l1];
  return (hi + 0.05) / (lo + 0.05);
}

function themeValue(name, theme) {
  const token = flat[name];
  if (!token) return undefined;
  if (typeof token === "object" && "light" in token) return token[theme];
  return token.value;
}

const results = [];
let failures = 0;
let skipped = 0;

for (const [group, level, min, enforced] of [
  ["text", "text", 4.5, true],
  ["non-text", "non-text", 3, true],
  ["advisory", "non-text", 3, false],
]) {
  for (const [fgName, bgName] of doc.contrast_pairs[group] ?? []) {
    for (const theme of ["light", "dark"]) {
      const fg = themeValue(fgName, theme);
      const bg = themeValue(bgName, theme);
      if (fg === undefined || bg === undefined) {
        results.push({ level, fgName, bgName, theme, ratio: null, ok: false, note: "token 缺失" });
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
      results.push({ level, fgName, bgName, theme, ratio, ok, min, enforced, note: enforced ? "" : "advisory（装饰性，不判定）" });
    }
  }
}

const pad = (s, n) => String(s).padEnd(n);
console.log(`MediaFlow Design tokens v${doc.version} — 双主题对比度验证（WCAG 2.2 AA）`);
console.log("=".repeat(96));
for (const r of results) {
  const mark = r.enforced === false ? "ADVI" : r.ok ? "PASS" : "FAIL";
  const val = r.ratio === null ? "  n/a" : r.ratio.toFixed(2).padStart(5);
  console.log(`${mark}  ${r.level === "text" ? "4.5" : "3.0"}  ${val}  ${pad(r.theme, 5)} ${r.fgName} on ${r.bgName}${r.note ? ` — ${r.note}` : ""}`);
}
console.log("=".repeat(96));
console.log(`${results.filter((r) => r.ok).length}/${results.length} 通过，${failures} 失败，${skipped} 跳过（渐变/复杂值不参与）。`);
process.exit(failures > 0 ? 1 : 0);
