#!/usr/bin/env node
// tokens/tokens.json（evergreen 基线）+ tokens/skins/*.json（皮肤层）
//   → site/assets/tokens.css：二维主题矩阵——[data-skin] × [data-theme] 组合属性选择器。
//   皮肤 = 颜色 token 双主题覆盖 + 可选风格参数（radius 等）；无 data-skin = evergreen。
// 零依赖：node:fs / node:path / node:url。

import { readFileSync, writeFileSync, mkdirSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tokensPath = join(here, "..", "tokens", "tokens.json");
const skinsDir = join(here, "..", "tokens", "skins");
const outDir = join(here, "..", "site", "assets");

const doc = JSON.parse(readFileSync(tokensPath, "utf8"));
const version = doc.version ?? "0.0.0";

const isColor = (v) => v && typeof v === "object" && "light" in v;

/** 遍历嵌套对象，产出扁平键。 */
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

/** 渲染一个变量组（selector 内），返回行数组。 */
const varLines = (pairs) => pairs.map(([name, value]) => `  --mfd-${name}: ${value};`);

// 基线（evergreen）
const flat = flatten(doc.semantic, "", Object.create(null));
const invariant = [];
const lightColors = [];
const darkColors = [];
for (const [name, value] of Object.entries(flat)) {
  if (isColor(value)) {
    lightColors.push([name, value.light]);
    darkColors.push([name, value.dark]);
  } else {
    invariant.push([name, value.value]);
  }
}

// 皮肤层
const skins = [];
for (const file of readdirSync(skinsDir).filter((f) => f.endsWith(".json")).sort()) {
  const skin = JSON.parse(readFileSync(join(skinsDir, file), "utf8"));
  const flatColor = flatten(skin.color ?? {}, "color", Object.create(null));
  const light = [];
  const dark = [];
  for (const [name, value] of Object.entries(flatColor)) {
    if (isColor(value)) {
      light.push([name, value.light]);
      dark.push([name, value.dark]);
    }
  }
  for (const [group, entries] of Object.entries(skin.style ?? {})) {
    for (const [key, value] of Object.entries(entries)) light.push([`${group}-${key}`, value]);
  }
  skins.push({ id: skin.name ?? file.replace(".json", ""), label: skin.label ?? skin.name, light, dark });
}

const out = [];
out.push(`/* MediaFlow Design tokens v${version} — 由 scripts/build-tokens.mjs 生成，勿手改。 */`);
out.push(`/* 唯一源：tokens/tokens.json（evergreen 基线）+ tokens/skins/*.json（皮肤层） */`);
out.push(`/* 二维主题：[data-skin=<skin>] × [data-theme=light|dark]；无 data-skin = evergreen。 */`);
out.push("");
out.push(`/* 皮肤清单：${skins.map((s) => `${s.id}（${s.label}）`).join(" · ")} */`);
out.push("");

// 1) 基线浅色 + 主题无关 token
out.push(":root, :root[data-skin=evergreen] {");
out.push("  color-scheme: light;");
out.push(...varLines(invariant));
out.push(...varLines(lightColors));
out.push("}");
out.push("");
// 2) 基线暗色（显式 + 跟随系统）
const darkBlock = (indent) => [
  ...darkColors.flatMap(([n, v]) => [`${indent}--mfd-${n}: ${v};`]),
];
out.push('[data-theme="dark"], [data-skin=evergreen][data-theme="dark"] {');
out.push("  color-scheme: dark;");
out.push(...varLines(darkColors));
out.push("}");
out.push("");
out.push("@media (prefers-color-scheme: dark) {");
out.push('  :root:not([data-theme]), :root[data-skin=evergreen]:not([data-theme]) {');
out.push(...darkBlock("    "), "  }", "}");
out.push("");

// 3) 各皮肤：浅色块 + 暗色块（显式 + 跟随系统）
for (const skin of skins) {
  out.push(`/* ---- skin: ${skin.id}（${skin.label}）---- */`);
  out.push(`:root[data-skin=${skin.id}] {`);
  out.push("  color-scheme: light;");
  out.push(...varLines(skin.light));
  out.push("}");
  out.push("");
  out.push(`[data-skin=${skin.id}][data-theme="dark"] {`);
  out.push("  color-scheme: dark;");
  out.push(...varLines(skin.dark));
  out.push("}");
  out.push("");
  out.push("@media (prefers-color-scheme: dark) {");
  out.push(`  :root[data-skin=${skin.id}]:not([data-theme]) {`);
  out.push(...skin.dark.map(([n, v]) => `    --mfd-${n}: ${v};`));
  out.push("  }", "}", "");
}

mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, "tokens.css"), out.join("\n"), "utf8");
// 站点自包含副本：基线 tokens.json + 皮肤清单
writeFileSync(join(outDir, "tokens.json"), readFileSync(tokensPath, "utf8"), "utf8");
writeFileSync(
  join(outDir, "skins.json"),
  JSON.stringify({ skins: [{ id: "evergreen", label: "常青", desc: "绿色系管理台基线（手工精调）" }, ...skins.map(({ id, label, ...rest }) => ({ id, label }))] }, null, 2),
  "utf8",
);

const count = {
  invariant: invariant.length,
  baselineDual: lightColors.length,
  skins: skins.map((s) => `${s.id}:${s.light.length}`),
};
console.log(`tokens v${version}: ${count.invariant} 不变 + ${count.baselineDual} 双主题基线 + 皮肤 ${JSON.stringify(count.skins)} -> ${join(outDir, "tokens.css")}`);
