# MediaFlow Design（设计系统工作台）

MediaFlow 的设计语言、设计 tokens、组件规范、交互模式与风格指南站的**生产地**。本目录是设计规范的唯一权威源；三个消费方（apps/web、m5-client 双端、未来 TV）通过采纳路线图消费产物，不直接依赖本目录进构建。

## 约束（先读）

- **零 npm 依赖**：风格指南站与生成脚本只用原生 ES Modules + Node 内置模块 + 本地 fixtures（沿用 `labs/web-prototype` 先例；构建环境网络受限）。
- **不接入根构建**：本目录自带 README 与运行说明，不修改 `pnpm-workspace.yaml`、根 `justfile`、`ci.yml`。
- **tokens 唯一源**：`tokens/tokens.json`；其他格式（CSS custom properties、三端映射）全部由 `scripts/` 生成，手改生成物无效。
- **双主题强制**：每个语义 token 必须同时具备浅色与暗色两份值；对比度基线 WCAG 2.2 AA（正文 ≥4.5:1、非文本 ≥3:1），`scripts/check-contrast.mjs` 自动验证。
- 正式应用不得 import 本目录任何文件（同 web-prototype 纪律）；采纳走"复制生成物 + 语义对齐"。

## 目录

```text
labs/design-system/
├── README.md          本文件
├── manifest.json      版本、产物清单、破坏性变更记录
├── tokens/            tokens.json（唯一源，D1 起）
├── docs/              设计语言与组件规格文档（D2 起）
├── scripts/           生成与检查脚本（tokens→CSS、对比度检查，D1 起）
└── site/              风格指南静态站（D4 起；python3 -m http.server 预览）
```

## 运行

```bash
# 风格指南站（D4 已就位）
python3 -m http.server 4174 --directory labs/design-system/site
# 浏览器打开 http://127.0.0.1:4174/ —— 侧栏底部三态切换浅/暗主题

# 生成 tokens 产物（tokens.css + 站点自包含 tokens.json 副本）
node labs/design-system/scripts/build-tokens.mjs
# 双主题对比度验证（WCAG 2.2 AA）
node labs/design-system/scripts/check-contrast.mjs
# 站点自动检查（语法/import/资源/var 引用/无字面色）
node labs/design-system/scripts/check-site.mjs
```

## 浏览器自检记录（2026-10-11）

- 自动检查全过 + 本地服务全资源 200；三视图（tokens/components/patterns）渲染齐全（组件页 12 区段实测）。
- 交互实测：六态切换（error 态切换成功）、主题三态（data-theme=dark、localStorage 持久化、aria-checked 联动）。
- 截图存档：`docs/screenshots/components-dark.png`（组件页暗色）、`docs/screenshots/patterns-light.png`（模式页浅色）。

## 状态

- v1.0.0 风格指南站 —— D4 完成（2026-10-11）：零依赖静态站（原生 ES Modules + fetch 本地 tokens.json）；三视图（tokens 数据驱动展示 / 12 组件 demo×状态矩阵 / 交互模式演示含可切换六态与审批链故事板）；浅/暗/跟随系统三态主题切换（防闪脚本 + localStorage）；自动检查 `scripts/check-site.mjs`。
- v1.0.0 tokens（D1）与组件规格（D2）、模式集（D3）见 manifest 与 docs/。
