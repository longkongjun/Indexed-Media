#!/usr/bin/env node
// 风格指南站自动检查：
// 1) 模块语法与 import 图解析（node --check + 手工解析 import 说明符）
// 2) index.html 引用的本地资源存在
// 3) site.css 与视图源码中引用的每个 var(--mfd-*) 在 tokens.css 中有定义
// 4) site.css / 视图源码无字面 hex 颜色（token 驱动纪律）
// 失败退出码 1。零依赖：node:fs / node:path / node:child_process。

import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const site = join(root, "site");
const failures = [];
const ok = (cond, msg) => { if (!cond) failures.push(msg); };

// ---------- 1. 模块语法 + import 图 ----------
const srcDir = join(site, "src");
const mjsFiles = [];
(function walk(dir) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p);
    else if (name.endsWith(".mjs")) mjsFiles.push(p);
  }
})(srcDir);

const importSpecs = [];
for (const file of mjsFiles) {
  try {
    execFileSync(process.execPath, ["--check", file], { stdio: "pipe" });
  } catch (error) {
    failures.push(`语法错误: ${file}: ${error.stderr}`);
  }
  const code = readFileSync(file, "utf8");
  for (const match of code.matchAll(/from\s+"([^"]+)"/g)) importSpecs.push({ file, spec: match[1] });
  for (const match of code.matchAll(/import\(\s*"([^"]+)"\s*\)/g)) importSpecs.push({ file, spec: match[1] });
}
for (const { file, spec } of importSpecs) {
  if (spec.startsWith("node:")) continue;
  ok(existsSync(join(dirname(file), spec)) || existsSync(join(site, spec)),
    `import 不可解析: ${file} -> ${spec}`);
}

// ---------- 2. index.html 本地资源 ----------
const html = readFileSync(join(site, "index.html"), "utf8");
for (const match of html.matchAll(/(?:src|href)="([^"#][^"]*)"/g)) {
  const ref = match[1];
  if (/^(https?:|data:|mailto:)/.test(ref)) continue;
  ok(existsSync(join(site, ref)), `index.html 引用缺失: ${ref}`);
}

// ---------- 3. var(--mfd-*) 引用完整性 ----------
const tokensCss = readFileSync(join(site, "assets", "tokens.css"), "utf8");
const defined = new Set([...tokensCss.matchAll(/--mfd-([a-z0-9-]+)\s*:/g)].map((m) => m[1]));
ok(defined.size > 90, `tokens.css 变量数异常: ${defined.size}`);

const cssCode = readFileSync(join(site, "assets", "site.css"), "utf8");
for (const match of cssCode.matchAll(/var\(--mfd-([a-z0-9-]+)/g)) {
  ok(defined.has(match[1]), `site.css 引用未定义 token: --mfd-${match[1]}`);
}
for (const file of mjsFiles) {
  const code = readFileSync(file, "utf8");
  for (const match of code.matchAll(/var\(--mfd-([a-z0-9-]+)\)/g)) {
    ok(defined.has(match[1]), `${file} 引用未定义 token: --mfd-${match[1]}`);
  }
  for (const match of code.matchAll(/--mfd-([a-z0-9-]+)\s*:/g)) {
    ok(defined.has(match[1]), `${file} 样式声明未定义 token: --mfd-${match[1]}`);
  }
}

// ---------- 4. 无字面 hex 颜色（token 驱动纪律） ----------
// 仅检查颜色值所在上下文：CSS 全文（去注释）与 mjs 内联 style 字符串；普通文案字符串不查（如 "任务 #1043"）。
function stripComments(code) {
  return code.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
}
ok([...stripComments(cssCode).matchAll(/#[0-9a-fA-F]{3,8}\b/g)].length === 0, "site.css 含字面 hex");
for (const file of mjsFiles) {
  const code = readFileSync(file, "utf8");
  const styleLiterals = [...code.matchAll(/style="([^"]*)"/g)].map((m) => m[1]).join(" ");
  const hits = [...styleLiterals.matchAll(/#[0-9a-fA-F]{3,8}\b/g)];
  ok(hits.length === 0, `${file} 内联样式含字面 hex: ${hits.map((h) => h[0]).join(", ")}`);
}

console.log(`check-site: ${mjsFiles.length} 模块 · ${importSpecs.length} import · ${defined.size} token 变量`);
if (failures.length) {
  console.error(`FAIL ×${failures.length}`);
  for (const f of failures) console.error(" -", f);
  process.exit(1);
}
console.log("PASS：语法/import/资源/var 引用/无字面色 全部通过");
