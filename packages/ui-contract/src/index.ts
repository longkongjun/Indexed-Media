/**
 * UI Contract 类型层：页面状态语义权威的 TypeScript 视图。
 *
 * 定本数据在 `../contract/*.json`（跨语言权威，KMP/文档侧可直接引用路径）；
 * 本模块提供消费方的类型与常量入口。结构漂移由三道防线拦截：
 * ① `scripts/check.mjs`（零依赖结构校验，含本文件字面量联合与 JSON 值域的比对）；
 * ② 本文件对 JSON 的直接赋值（形状漂移导致 tsc 失败）；
 * ③ apps/web 的漂移防线测试（TaskCenterState ↔ 权威 JSON 双向对齐）。
 */
import copyTemplatesDocument from "../contract/copy-templates.json";
import displayStateMappingDocument from "../contract/display-state-mapping.json";
import pageStatesDocument from "../contract/page-states.json";

/** 页面状态 id 值域（五态；TaskCenterState/PageState 的 kind）。 */
export type PageStateId = "loading" | "content" | "empty" | "offline" | "error";

/** 展示态 id 值域（设计系统六态容器 + Success）。 */
export type DisplayStateId = "loading" | "empty" | "error" | "offline" | "conflict" | "partial" | "success";

/** 展示态驱动层：page-state（数据生命周期直映射）或 operation（操作结果叠加）。 */
export type DisplayStateDriver = "page-state" | "operation";

/**
 * 单个页面状态定义。
 *
 * `id` 值域即 {@link PageStateId}（JSON 导入会拓宽字面量，值域一致性由 check.mjs 钉住）。
 */
export interface PageStateDefinition {
  id: string;
  /** 该态是否为 stale 载体（仅 loading/offline/error 为 true）。 */
  stale: boolean;
  meaning: string;
  stale_semantics: string;
  exit: string;
}

export interface PageStateImplementation {
  platform: string;
  path: string;
  symbol: string;
}

export interface PageStatesDocument {
  contract: string;
  version: string;
  title: string;
  layer: string;
  implementations: PageStateImplementation[];
  states: PageStateDefinition[];
  invariants: string[];
}

/**
 * 单个展示态的映射定义。
 *
 * `id` 值域即 {@link DisplayStateId}；page-state 驱动的展示态 `page_states` 非空且
 * 逐一映射页面态，operation 驱动的为 null 并以 `operation_semantics` 说明。
 */
export interface DisplayStateDefinition {
  id: string;
  driven_by: string;
  page_states: string[] | null;
  operation_semantics: string | null;
}

export interface DisplayStateMappingDocument {
  contract: string;
  version: string;
  title: string;
  layers: Record<string, string>;
  display_states: DisplayStateDefinition[];
  composition_rules: string[];
  stale_presentation: { opacity: string; interaction: string; note: string };
}

export interface StateCopyTemplate {
  id: string;
  template: string;
  example?: string;
  primary_action?: string;
}

export interface CopyTemplatesDocument {
  contract: string;
  version: string;
  title: string;
  error_template: { pattern: string; elements: Record<string, string>; example: string };
  state_templates: StateCopyTemplate[];
  forbidden: string[];
  word_lists: { status: string[]; actions: string[] };
  format_rules: string[];
}

/** 页面状态权威（五态 + stale）。 */
export const pageStates: PageStatesDocument = pageStatesDocument;

/** 页面五态 ↔ 设计系统状态容器的分层映射。 */
export const displayStateMapping: DisplayStateMappingDocument = displayStateMappingDocument;

/** 错误与状态文案模板索引（模板、禁区、词表）。 */
export const copyTemplates: CopyTemplatesDocument = copyTemplatesDocument;
