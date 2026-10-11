#!/usr/bin/env node
// UI Contract 定本结构校验（零依赖）。
// 校验三层：① JSON 结构完备性；② 跨文件引用一致性；③ src/index.ts 字面量联合与 JSON 值域一致。
// 入口：`just check-ui-contract`（与 `tsc --noEmit` 组成完整检查）。
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const failures = [];

const fail = (message) => failures.push(message);

const readJson = (name) => {
  try {
    return JSON.parse(readFileSync(join(root, "contract", name), "utf8"));
  } catch (error) {
    fail(`${name} 读取/解析失败：${error.message}`);
    return null;
  }
};

const expectString = (value, label) => {
  if (typeof value !== "string" || value.trim() === "") fail(`${label} 必须是非空字符串`);
};

const expectStringArray = (value, label) => {
  if (!Array.isArray(value) || value.length === 0 || value.some((item) => typeof item !== "string" || item.trim() === "")) {
    fail(`${label} 必须是非空字符串数组`);
  }
};

const PAGE_STATE_IDS = ["loading", "content", "empty", "offline", "error"];
const STALE_CARRIER_IDS = ["loading", "offline", "error"];
const PAGE_STATE_DRIVEN_DISPLAY_IDS = ["loading", "empty", "error", "offline"];
const OPERATION_DRIVEN_DISPLAY_IDS = ["conflict", "partial", "success"];
const DISPLAY_STATE_IDS = [...PAGE_STATE_DRIVEN_DISPLAY_IDS, ...OPERATION_DRIVEN_DISPLAY_IDS];
const TEMPLATE_IDS = ["empty-list", "empty-filter", "offline", "conflict", "partial", "success", "confirm-destructive"];
const STATUS_WORDS = ["完成", "成功", "待确认", "部分成功", "降级", "失败", "冲突", "运行中", "排队", "未开始", "已停用", "跳过"];
const ACTION_WORDS = ["重试", "重新计算计划", "修正规则", "确认候选", "转为通用视频", "批准", "拒绝", "隔离", "回滚"];
const REQUIRED_CONTRACT_FIELDS = ["contract", "version", "title"];

// ① page-states.json：五态完备、stale 载体正确、实现锚点在位。
const pageStates = readJson("page-states.json");
if (pageStates) {
  for (const field of REQUIRED_CONTRACT_FIELDS) expectString(pageStates[field], `page-states.json ${field}`);
  expectStringArray(pageStates.invariants, "page-states.json invariants");
  if (!Array.isArray(pageStates.states)) {
    fail("page-states.json states 必须是数组");
  } else {
    const ids = pageStates.states.map((state) => state.id);
    const missing = PAGE_STATE_IDS.filter((id) => !ids.includes(id));
    const extra = ids.filter((id) => !PAGE_STATE_IDS.includes(id));
    if (missing.length > 0) fail(`page-states.json 缺少状态：${missing.join(", ")}`);
    if (extra.length > 0) fail(`page-states.json 多出状态：${extra.join(", ")}`);
    for (const state of pageStates.states) {
      for (const field of ["id", "meaning", "stale_semantics", "exit"]) expectString(state[field], `page-states.json states[${state.id}].${field}`);
      const expectedStale = STALE_CARRIER_IDS.includes(state.id);
      if (state.stale !== expectedStale) {
        fail(`page-states.json states[${state.id}].stale 应为 ${expectedStale}`);
      }
    }
  }
  if (!Array.isArray(pageStates.implementations) || !pageStates.implementations.some((entry) => entry.platform === "web") || !pageStates.implementations.some((entry) => entry.platform === "kmp-sdk")) {
    fail("page-states.json implementations 必须同时覆盖 web 与 kmp-sdk 锚点");
  }
}

// ② display-state-mapping.json：七展示态、驱动层正确、页面态映射恰好一次。
const mapping = readJson("display-state-mapping.json");
if (mapping) {
  for (const field of [...REQUIRED_CONTRACT_FIELDS]) expectString(mapping[field], `display-state-mapping.json ${field}`);
  expectStringArray(mapping.composition_rules, "display-state-mapping.json composition_rules");
  if (!Array.isArray(mapping.display_states)) {
    fail("display-state-mapping.json display_states 必须是数组");
  } else {
    const ids = mapping.display_states.map((state) => state.id);
    const missing = DISPLAY_STATE_IDS.filter((id) => !ids.includes(id));
    const extra = ids.filter((id) => !DISPLAY_STATE_IDS.includes(id));
    if (missing.length > 0) fail(`display-state-mapping.json 缺少展示态：${missing.join(", ")}`);
    if (extra.length > 0) fail(`display-state-mapping.json 多出展示态：${extra.join(", ")}`);
    const mappedPageStates = [];
    for (const state of mapping.display_states) {
      expectString(state.id, "display-state-mapping.json display_states[].id");
      const pageDriven = state.driven_by === "page-state";
      const operationDriven = state.driven_by === "operation";
      if (!pageDriven && !operationDriven) fail(`display-state-mapping.json display_states[${state.id}].driven_by 必须是 page-state 或 operation`);
      if (pageDriven) {
        if (!PAGE_STATE_DRIVEN_DISPLAY_IDS.includes(state.id)) fail(`display_states[${state.id}] 不应标记为 page-state 驱动`);
        if (!Array.isArray(state.page_states) || state.page_states.length !== 1) fail(`display_states[${state.id}] page_states 应恰有一个页面态`);
        else mappedPageStates.push(...state.page_states);
        if (state.operation_semantics !== null) fail(`display_states[${state.id}] page-state 驱动时 operation_semantics 应为 null`);
      }
      if (operationDriven) {
        if (!OPERATION_DRIVEN_DISPLAY_IDS.includes(state.id)) fail(`display_states[${state.id}] 不应标记为 operation 驱动`);
        if (state.page_states !== null) fail(`display_states[${state.id}] operation 驱动时 page_states 应为 null`);
        expectString(state.operation_semantics, `display_states[${state.id}].operation_semantics`);
      }
    }
    // content 不映射容器；其余四态恰好出现一次。
    const expectedMapped = PAGE_STATE_DRIVEN_DISPLAY_IDS;
    const sortedMapped = [...mappedPageStates].sort();
    const sortedExpected = [...expectedMapped].sort();
    if (JSON.stringify(sortedMapped) !== JSON.stringify(sortedExpected)) {
      fail(`页面态→展示态映射应为 ${sortedExpected.join(", ")}，实际 ${sortedMapped.join(", ") || "(空)"}`);
    }
  }
  if (!mapping.stale_presentation || typeof mapping.stale_presentation.opacity !== "string") {
    fail("display-state-mapping.json stale_presentation.opacity 缺失");
  }
}

// ③ copy-templates.json：三要素模板、七态模板、禁区、词表。
const copy = readJson("copy-templates.json");
if (copy) {
  for (const field of REQUIRED_CONTRACT_FIELDS) expectString(copy[field], `copy-templates.json ${field}`);
  if (!copy.error_template) {
    fail("copy-templates.json error_template 缺失");
  } else {
    expectString(copy.error_template.pattern, "copy-templates.json error_template.pattern");
    expectString(copy.error_template.example, "copy-templates.json error_template.example");
    for (const placeholder of ["{对象}", "{原因摘要}", "下一步"]) {
      if (!copy.error_template.pattern.includes(placeholder)) fail(`error_template.pattern 缺少要素 ${placeholder}`);
    }
  }
  if (!Array.isArray(copy.state_templates)) {
    fail("copy-templates.json state_templates 必须是数组");
  } else {
    const ids = copy.state_templates.map((template) => template.id);
    const missing = TEMPLATE_IDS.filter((id) => !ids.includes(id));
    if (missing.length > 0) fail(`copy-templates.json state_templates 缺少：${missing.join(", ")}`);
    for (const template of copy.state_templates) {
      expectString(template.id, "state_templates[].id");
      expectString(template.template, `state_templates[${template.id}].template`);
    }
  }
  expectStringArray(copy.forbidden, "copy-templates.json forbidden");
  expectStringArray(copy.format_rules, "copy-templates.json format_rules");
  if (!copy.word_lists || JSON.stringify(copy.word_lists.status) !== JSON.stringify(STATUS_WORDS)) {
    fail(`copy-templates.json word_lists.status 必须与状态词表逐项一致：${STATUS_WORDS.join("、")}`);
  }
  if (!copy.word_lists || !ACTION_WORDS.every((word) => copy.word_lists.actions.includes(word))) {
    fail(`copy-templates.json word_lists.actions 必须包含核心动作词：${ACTION_WORDS.join("、")}`);
  }
}

// ④ src/index.ts 字面量联合与 JSON 值域一致（防 TS 镜像漂移）。
const indexSource = readFileSync(join(root, "src", "index.ts"), "utf8");
const extractUnion = (typeName) => {
  const match = indexSource.match(new RegExp(`export type ${typeName} = ([^;]+);`));
  if (!match) return null;
  return match[1]
    .split("|")
    .map((part) => part.trim().replaceAll('"', "").replaceAll("'", ""))
    .filter(Boolean);
};
const pageUnion = extractUnion("PageStateId");
if (!pageUnion) fail("src/index.ts 缺少 `export type PageStateId` 声明");
else if (JSON.stringify([...pageUnion].sort()) !== JSON.stringify([...PAGE_STATE_IDS].sort())) {
  fail(`src/index.ts PageStateId（${pageUnion.join(" | ")}）与权威五态（${PAGE_STATE_IDS.join(" | ")}）不一致`);
}
const displayUnion = extractUnion("DisplayStateId");
if (!displayUnion) fail("src/index.ts 缺少 `export type DisplayStateId` 声明");
else if (JSON.stringify([...displayUnion].sort()) !== JSON.stringify([...DISPLAY_STATE_IDS].sort())) {
  fail(`src/index.ts DisplayStateId（${displayUnion.join(" | ")}）与展示态值域（${DISPLAY_STATE_IDS.join(" | ")}）不一致`);
}

if (failures.length > 0) {
  console.error(`ui-contract 结构校验失败（${failures.length} 项）：`);
  for (const message of failures) console.error(`  - ${message}`);
  process.exit(1);
}
console.log("ui-contract 结构校验通过：page-states / display-state-mapping / copy-templates / TS 类型镜像一致。");
