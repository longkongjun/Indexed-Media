// 交互模式页：可操作的模式演示（六态切换、审批链故事板、恢复动作表、文案模板、主题切换说明）。

import { button, badge, panel, section } from "../demo-kit.mjs";

const STATES = {
  loading: { glyph: "◌", spin: true, title: "正在加载任务列表…", body: "保留导航与骨架，避免跳动。", action: null },
  empty: { glyph: "▢", title: "暂无正式媒体记录", body: "完成待确认任务后即可建立正式记录。", action: "查看待确认任务" },
  error: { glyph: "{!}", title: "任务列表加载失败", body: "任务列表在获取阶段失败：连接超时。下一步：稍后重试。", action: "重试" },
  offline: { glyph: "⌁", title: "无法连接 MediaFlow 服务", body: "正在显示 5 分钟前的数据；需服务端确认的操作已禁用。", action: "重试" },
  conflict: { glyph: "⚖", title: "计划与已存在文件冲突", body: "inbox/dune2-cn.mkv 与目标指纹相同。下一步：修正规则或目标后重新计算计划。", action: "查看冲突详情" },
  partial: { glyph: "◐", title: "整理部分完成", body: "3 项已完成，1 项未完成：NFO 写入失败。下一步：只重试未完成步骤。", action: "重试未完成步骤" },
  success: { glyph: "✓", title: "整理完成", body: "文件操作、NFO 与媒体记录均完成。", action: "查看结果" },
};

function demoStateToggle() {
  const toggleBar = document.createElement("div");
  toggleBar.className = "state-toggle";
  toggleBar.setAttribute("role", "group");
  toggleBar.setAttribute("aria-label", "选择六态演示");
  const stage = document.createElement("div");
  stage.setAttribute("aria-live", "polite");
  const show = (state) => {
    const cfg = STATES[state];
    const box = document.createElement("div");
    box.className = "state-box";
    box.dataset.state = state;
    box.style.maxWidth = "26rem";
    box.innerHTML = `
      <span class="state-icon${cfg.spin ? " spin" : ""}" aria-hidden="true">${cfg.glyph}</span>
      <span class="state-title">${cfg.title}</span>
      <span class="state-body">${cfg.body}</span>`;
    if (cfg.action) {
      const actions = document.createElement("div");
      actions.className = "state-actions";
      actions.append(button(cfg.action, "primary", "btn-compact"));
      box.append(actions);
    }
    stage.replaceChildren(box);
  };
  for (const [state, cfg] of Object.entries(STATES)) {
    const b = button(cfg.title.slice(0, 6), "secondary", "btn-compact");
    b.textContent = { loading: "加载", empty: "空", error: "错误", offline: "离线", conflict: "冲突", partial: "部分", success: "成功" }[state];
    b.addEventListener("click", () => show(state));
    toggleBar.append(b);
  }
  show("loading");
  return section("p-states", "六态容器（可切换）", "状态→展示重点→唯一主操作；点按钮切换观察底色/描边/主操作变化（双主题均可查）", "state-container", toggleBar, stage);
}

function demoApprovalFlow() {
  const wrap = document.createElement("div");
  const steps = [
    { tag: "① 会话摘要", node: null, html: '<div class="msg msg-ai"><span class="role">AI 助手</span>task-1043 的计划已就绪：2 项移动、1 项隔离。需要你在审批页核对完整影响后批准。<br><a class="msg-link" href="#/patterns">任务 #1043 →</a></div>' },
    { tag: "② 通知 + 角标", node: null, html: '<div class="notice-item unread"><span class="notice-kind">待审确认</span><p style="margin:.35rem 0">task-1043 的整理计划等待审批</p><span class="notice-time">刚刚</span></div>' },
    { tag: "③ 审批横幅", node: null, html: '<div class="approval-banner" role="alert"><span aria-hidden="true">⚖</span><div class="ab-text"><div class="ab-title">1 个整理计划等待审批</div><div class="ab-summary">计划 v7：2 项移动 · 1 项隔离</div></div></div>' },
    { tag: "④ 完整影响 → 批准", node: null, html: '<div class="plan-row"><span class="badge badge-info"><i class="glyph" aria-hidden="true">→</i>移动</span><span class="plan-path">inbox/dune2.mkv</span><span class="plan-arrow">→</span><span class="plan-path">媒体库/电影/沙丘 2 (2024)/沙丘 2 (2024).mkv</span></div><div class="plan-actions"><button class="btn btn-secondary">拒绝</button><button class="btn btn-primary">批准并执行计划 v7</button></div>' },
    { tag: "⑤ 结果（SSE 同步）", node: null, html: '<div class="state-box" data-state="success" style="max-width:26rem"><span class="state-icon" aria-hidden="true">✓</span><span class="state-title">整理完成</span><span class="state-body">文件操作、NFO 与媒体记录均完成；时间线与 journal 已留痕。</span></div>' },
  ];
  for (const step of steps) {
    const label = document.createElement("div");
    label.className = "step-pill now";
    label.style.margin = "0.5rem 0 0.25rem";
    label.textContent = step.tag;
    const body = document.createElement("div");
    body.innerHTML = step.html;
    wrap.append(label, body);
  }
  return section("p-approval", "审批链端到端（故事板）", "会话摘要→通知→横幅→完整影响→批准→结果；会话内无审批按钮；版本绑定防过期", "approval-flow", panel(wrap));
}

function demoRecovery() {
  const rows = [
    ["待确认", "候选、置信度、依据、原文件未变", "选择候选 / 转通用视频 → 重算计划"],
    ["冲突", "来源、目标、规则缺口", "修正规则或目标 → 重算计划"],
    ["失败", "失败阶段、已产生结果、未执行动作", "从安全检查点重试"],
    ["部分成功", "已完成与未完成分组", "只重试未完成步骤"],
    ["等待审批", "计划摘要 + 版本", "查看完整计划并审批"],
  ];
  const tbl = document.createElement("table");
  tbl.className = "data-table";
  tbl.innerHTML = `<thead><tr><th>状态</th><th>展示重点</th><th>唯一主操作</th></tr></thead><tbody>${rows
    .map((r) => `<tr><td>${badge({ 待确认: "◐", 冲突: "▲", 失败: "▲", 部分成功: "◐", 等待审批: "⚖" }[r[0]], r[0], ["待确认", "部分成功", "等待审批"].includes(r[0]) ? "warning" : "danger").outerHTML}</td><td>${r[1]}</td><td>${r[2]}</td></tr>`)
    .join("")}</tbody>`;
  return section("p-recovery", "恢复动作模式", "每状态一个主操作；重试不重复已安全完成的整理；无绕过授权的继续按钮", "recovery-actions", panel(tbl));
}

function demoCopy() {
  const p = panel();
  p.innerHTML = `
    <p><strong>三要素模板</strong>：「{对象}在{阶段}失败：{原因摘要}。下一步：{动作}。」</p>
    <div class="msg msg-ai"><span class="role">示例（Error）</span>task-1042 在文件操作阶段失败：目标路径已存在（conflict）。下一步：<a class="msg-link" href="#/patterns">修正规则或目标后重新计算计划</a>。</div>
    <div class="msg msg-ai"><span class="role">示例（Empty）</span>暂无正式媒体记录。完成待确认任务后即可建立。</div>
    <div class="msg msg-ai"><span class="role">示例（Offline）</span>无法连接 MediaFlow 服务。正在显示 5 分钟前的数据。</div>`;
  return section("p-copy", "文案模板", "对象+原因+下一步；术语与 wiki 术语表一致；动作词无同义变体", "copy-patterns", p);
}

function demoTheme() {
  const p = panel();
  p.innerHTML = `
    <p><strong>本站右上（侧栏底）控件即规范实现</strong>：跟随系统 / 浅色 / 暗色 三态分段控件（role=radiogroup）。</p>
    <ul>
      <li>跟随系统：无 <code>data-theme</code> 属性，由 <code>prefers-color-scheme</code> 驱动（改系统主题实时跟随）。</li>
      <li>手动选择：写 <code>data-theme</code> + <code>localStorage['mfd-theme']</code>；刷新不闪（index.html 内联防闪脚本）。</li>
      <li>过渡 400ms 仅颜色属性；<code>prefers-reduced-motion</code> 即切；图片零处理。</li>
      <li>v1 各端本地记忆不同步；未来账号级同步仅同步手动选择。</li>
    </ul>`;
  return section("p-theme", "主题切换", "三态模型 + 防闪 + 本地记忆；切换主题观察本页全部 demo", "theme-switching", p);
}

export async function renderPatternsView() {
  const frag = document.createDocumentFragment();
  const h1 = document.createElement("h1");
  h1.className = "page-title";
  h1.textContent = "交互模式";
  const desc = document.createElement("p");
  desc.className = "page-desc";
  desc.textContent = "模式演示与验收锚点；完整规则见 docs/patterns/ 对应文档";
  frag.append(h1, desc);
  frag.append(demoStateToggle(), demoApprovalFlow(), demoRecovery(), demoCopy(), demoTheme());
  return frag;
}
