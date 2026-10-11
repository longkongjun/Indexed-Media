#!/usr/bin/env node
// tokens/tokens.json → site/assets/tokens.css（唯一生成链；手改产物无效）
// 产物结构：:root = 主题无关 token（尺寸/字体/动效）+ 浅色颜色值；
// [data-theme="dark"] = 暗色覆盖；@media (prefers-color-scheme: dark) :root:not([data-theme]) = 跟随系统。
// 零依赖：仅用 node:fs / node:path / node:url。

import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tokensPath = join(here, "..", "tokens", "tokens.json");
const outDir = join(here, "..", "site", "assets");
const outPath = join(outDir, "tokens.css");

const doc = JSON.parse(readFileSync(tokensPath, "utf8"));
const version = doc.version ?? "0.0.0";

/** 遍历嵌套对象，产出扁平键（数组路径以 '-' 连接）。 */
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
const lines = [];
const cssVar = (name, value) => `  --mfd-${name}: ${value};`;

// 主题无关 token（value 标量）与浅色颜色（light/dark 双值取 light）
const invariant = [];
const lightColors = [];
const darkColors = [];
for (const [name, value] of Object.entries(flat)) {
  if (value && typeof value === "object" && "light" in value) {
    lightColors.push(cssVar(name, value.light));
    darkColors.push(cssVar(name, value.dark));
  } else {
    invariant.push(cssVar(name, value.value));
  }
}

lines.push(`/* MediaFlow Design tokens v${version} — 由 scripts/build-tokens.mjs 生成，勿手改。 */`);
lines.push(`/* 唯一源：labs/design-system/tokens/tokens.json */`);
lines.push("");
lines.push(":root {");
lines.push("  color-scheme: light;");
lines.push(...invariant);
lines.push(...lightColors);
lines.push("}");
lines.push("");
lines.push('[data-theme="dark"] {');
lines.push("  color-scheme: dark;");
lines.push(...darkColors);
lines.push("}");
lines.push("");
lines.push("/* 无显式选择时跟随系统 */");
lines.push("@media (prefers-color-scheme: dark) {");
lines.push('  :root:not([data-theme]) {');
lines.push(...darkColors);
lines.push("  }");
lines.push("}");
lines.push("");

mkdirSync(outDir, { recursive: true });
writeFileSync(outPath, lines.join("\n"), "utf8");
// 站点自包含：tokens.json 副本供风格指南运行时读取（服务目录可为 site/ 本身）
writeFileSync(join(outDir, "tokens.json"), readFileSync(tokensPath, "utf8"), "utf8");

const count = { invariant: invariant.length, themed: lightColors.length };
console.log(`tokens v${version}: ${count.invariant} invariant + ${count.themed} dual-theme tokens -> ${outPath}`);
