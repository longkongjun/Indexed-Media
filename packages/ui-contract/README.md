# UI Contract

## 目标

定义跨端 UI 预期，不实现正式 UI。

## 职责

公共 UI 契约：设计 token、页面状态、交互语义、无障碍和覆盖矩阵。

## 允许依赖

`contracts`；绝不能依赖 `apps` 或 `labs`。

## 非目标

应用专属 UI、生命周期代码、平台组件实现以及对任何应用或实验项目的依赖。

## 状态

已初始化（页面状态语义权威，2026-10-11）。

## 结构

| 路径 | 内容 |
|---|---|
| `contract/page-states.json` | **页面状态权威**：五态（loading/content/empty/offline/error）+ stale 语义、不变量、实现锚点（web `TaskCenterState` / kmp-sdk `PageState`）。 |
| `contract/display-state-mapping.json` | **分层映射**：页面五态 ↔ 设计系统六态容器（+Success）；page-state 直映射与 operation 叠加（conflict/partial/success）的组合规则。 |
| `contract/copy-templates.json` | **文案模板索引**：错误三要素模板（对象+原因+下一步）、各态文案模板、禁区、状态/动作词表。 |
| `src/index.ts` | TypeScript 类型层（字面量联合 + 文档类型 + 定本常量），供 TS 消费方引用。 |
| `scripts/check.mjs` | 零依赖结构校验：JSON 完备性、跨文件一致性、TS 镜像与 JSON 值域比对。 |

三份 JSON 是跨语言定本（KMP/文档侧直接引用路径）；引用链上的实现（web、kmp-sdk）以注释指向本包。
展示态的视觉与交互细则（图标、底色、主操作）由 `labs/design-system/docs/components/` 规格承载，本包只钉语义。

## 验证

`just check-ui-contract`（= 根 tsc `--noEmit` + `scripts/check.mjs`）；web 侧另有漂移防线测试
（`apps/web/tests/ui-contract.test.ts`，TaskCenterState ↔ 权威 JSON 双向对齐）。
