// 组件规格页：12 件组件 demo × 状态矩阵，全部以 token 驱动的 site.css 类渲染（双主题可查）。

import { section, panel, note, badge, button } from "../demo-kit.mjs";

/* ---------- 各组件 demo ---------- */

function demoButton() {
  const row1 = document.createElement("div");
  row1.className = "demo-row";
  row1.append(
    button("重新计算计划"),
    button("上一步", "secondary"),
    button("删除收件目录", "danger"),
    button("查看任务", "link"),
  );
  const row2 = document.createElement("div");
  row2.className = "demo-row";
  row2.append(
    button('<span class="spin" aria-hidden="true"></span>保存中…', "primary"),
    button("批准计划 v7", "primary", "btn-compact"),
    Object.assign(button("不可用", "secondary"), { disabled: true }),
  );
  return section(
    "c-button", "按钮与操作", "主/次/危险/链接四型；每视图最多一个 primary；加载态锁定宽度",
    "button",
    panel(row1, row2, note("hover/active/focus-visible 状态切到组件规格文档查矩阵；此处演示默认与加载/禁用态")),
  );
}

function demoForm() {
  const p = panel();
  p.innerHTML = `
    <div class="form-demo">
      <div class="field">
        <label for="d-inbox">收件目录路径</label>
        <input id="d-inbox" value="/vol1/media/inbox" />
        <p class="help">授权范围内的绝对路径；只验证与移动文件，不会自动整理。</p>
      </div>
      <div class="field">
        <label for="d-lib">整理目标库</label>
        <select id="d-lib"><option>电影库（/vol1/media/电影）</option><option>剧集库（/vol1/media/剧集）</option></select>
      </div>
      <div class="field">
        <label for="d-bad">资源库名称</label>
        <input id="d-bad" value="电影" aria-invalid="true" />
        <p class="err">⚠ 名称与已有资源库冲突（resource.conflict）。下一步：修改名称后重试，草稿已保留。</p>
      </div>
      <label class="check-line"><input type="checkbox" checked /> 启用自动执行（计划确认后无需逐次审批）</label>
    </div>`;
  return section("c-form", "输入与表单", "标签-帮助-错误结构；409 冲突草稿保留；只读值用等宽井", "form", p);
}

function demoTable({ fixtures }) {
  const tbl = document.createElement("table");
  tbl.className = "data-table";
  tbl.innerHTML = `
    <caption class="visually-hidden" style="position:absolute;left:-9999px">能力事件只读流</caption>
    <thead><tr><th>序号</th><th>事件</th><th>关联任务</th><th>状态</th></tr></thead>
    <tbody></tbody>`;
  const tbody = tbl.querySelector("tbody");
  for (const ev of fixtures.events) {
    const tr = document.createElement("tr");
    tr.innerHTML = `
      <td class="mono">#${ev.sequence}</td>
      <td>${ev.label}</td>
      <td><button type="button" class="btn btn-link">${ev.taskId.slice(0, 8)}…</button></td>
      <td></td>`;
    tr.lastElementChild.append(ev.sequence === 123 ? badge("◐", "待审批", "warning") : badge("◆", "完成", "success"));
    tbody.append(tr);
  }
  const pager = document.createElement("div");
  pager.className = "pager";
  pager.append(button("上一页", "secondary", "btn-compact"), Object.assign(button("下一页", "secondary", "btn-compact"), { disabled: true }), Object.assign(document.createElement("span"), { textContent: "已显示 3 项" }));
  return section("c-table", "表格与列表", "表头 selected 底；行分隔装饰级；游标分页 + sr-only caption", "table-list", panel(tbl, pager));
}

function demoMediaWall({ fixtures }) {
  const wall = document.createElement("div");
  wall.className = "wall";
  for (const m of fixtures.media) {
    const card = document.createElement("a");
    card.className = "media-card";
    card.href = "#/components";
    card.setAttribute("aria-label", `《${m.title}》${m.year ? `(${m.year})` : ""} 详情`);
    card.innerHTML = `
      <div class="poster">
        ${m.registered ? '<div class="art"><span>海报</span></div>' : '<span aria-hidden="true">▢</span>'}
        <span class="corner">${m.kind}</span>
      </div>
      <div class="cap">${m.title}
        <span class="meta">${[m.year, m.registered ? "▣ 已登记" : "□ 未登记"].filter(Boolean).join(" · ")}</span>
      </div>`;
    wall.append(card);
  }
  return section(
    "c-media", "媒体卡片 🔆", "海报墙：图片零主题处理；暗色靠容器描边分层；登记状态 ▣/□ 双编码",
    "media-card",
    panel(wall, note("demo 海报为渐变假图（占位演示）；真实海报在任何主题下原图呈现")),
  );
}

function demoBadge() {
  const row = document.createElement("div");
  row.className = "demo-row";
  row.append(
    badge("◆", "完成", "success"),
    badge("◐", "部分成功", "warning"),
    badge("▲", "失败", "danger"),
    badge("◌", "运行中", "info"),
    badge("○", "未开始", "neutral"),
    badge("●", "正常", "success", true),
    badge("◐", "降级", "warning", true),
    badge("○", "不可用", "neutral", true),
  );
  return section("c-badge", "状态徽章", "形状符 + 中文标签双编码；tone 五值；compact 无底变体", "status-badge", panel(row));
}

function demoTimeline() {
  const stages = [
    { stage: "发现", state: "done", glyph: "●", meta: "14:02 · 已发现 2 个文件" },
    { stage: "识别", state: "done", glyph: "●", meta: "14:03 · 候选《沙丘 2》0.92" },
    { stage: "文件操作", state: "failed", glyph: "▲", meta: "14:05 · 失败：目标路径已存在", detail: "已完成：0 项 · 未执行：2 项 —— <span class='mono'>inbox/dune2-cn.mkv → 隔离区/冲突/</span>" },
    { stage: "NFO", state: "todo", glyph: "○", meta: "未开始" },
    { stage: "完成", state: "todo", glyph: "○", meta: "未开始" },
  ];
  const ol = document.createElement("ol");
  ol.className = "timeline";
  ol.append(...stages.map((s) => {
    const li = document.createElement("li");
    if (s.state === "done") li.className = "done";
    li.innerHTML = `
      <i class="tl-node ${s.state === "failed" ? "" : ""}" aria-hidden="true">${s.glyph}</i>
      <div class="tl-head"><span class="tl-stage">${s.stage}</span><span class="tl-meta">${s.meta}</span></div>
      ${s.detail ? `<div class="tl-detail">${s.detail}<br>唯一主操作：${""}</div>` : ""}`;
    if (s.detail) {
      const btn = button("从安全检查点重试", "primary", "btn-compact");
      li.querySelector(".tl-detail").append(btn);
    }
    return li;
  }));
  return section("c-timeline", "任务时间线", "阶段推进 + 问题节点默认展开 + 唯一恢复动作", "task-timeline", panel(ol));
}

function demoPlan({ fixtures }) {
  const plan = fixtures.plan;
  const p = panel();
  p.innerHTML = `
    <div class="plan-head">
      <strong>整理计划 v${plan.version}</strong>
      <span class="plan-ver">task-${plan.task.slice(5)} · expected_revision ${plan.version}</span>
      <span>共 ${plan.groups.flatMap((g) => g.ops).length} 项操作</span>
    </div>`;
  for (const group of plan.groups) {
    const head = document.createElement("div");
    head.className = "plan-group";
    head.textContent = `→ ${group.target}`;
    p.append(head);
    for (const op of group.ops) {
      const row = document.createElement("div");
      row.className = "plan-row";
      row.append(badge(op.glyph, op.label, op.tone));
      const from = document.createElement("span");
      from.className = "plan-path";
      from.textContent = op.from;
      const arrow = document.createElement("span");
      arrow.className = "plan-arrow";
      arrow.textContent = "→";
      const to = document.createElement("span");
      to.className = "plan-path";
      to.textContent = op.to;
      row.append(from, arrow, to);
      if (op.conflict) row.append(badge("⚠", op.conflict, "warning"));
      p.append(row);
    }
  }
  const actions = document.createElement("div");
  actions.className = "plan-actions";
  actions.append(button("拒绝", "secondary"), button(`批准并执行计划 v${plan.version}`, "primary"));
  p.append(actions, note("含冲突行的计划：批准禁用，先修正规则或目标（demo 未含此态切换；见规格 §2）"));
  return section("c-plan", "计划变更预览 🔆", "完整影响清单：来源→目标+操作类型；批准绑定计划版本；含冲突即阻止", "plan-preview", p);
}

function demoApproval() {
  const p = panel();
  p.innerHTML = `
    <div class="approval-banner" role="alert">
      <span aria-hidden="true">⚖</span>
      <div class="ab-text">
        <div class="ab-title">1 个整理计划等待审批</div>
        <div class="ab-summary">task-1043 · 计划 v7：2 项移动 · 1 项隔离</div>
      </div>
    </div>`;
  p.querySelector(".approval-banner").append(button("查看完整计划并审批", "primary"), button("稍后处理", "link"));
  return section("c-approval", "审批横幅", "唯一权威审批入口；版本绑定；会话内不出现审批按钮", "approval-banner", p);
}

function demoSession({ fixtures }) {
  const box = document.createElement("div");
  box.className = "session";
  for (const m of fixtures.session) {
    const div = document.createElement("div");
    div.className = `msg ${m.role === "user" ? "msg-user" : "msg-ai"}`;
    div.innerHTML = `<span class="role">${m.role === "user" ? "你" : "AI 助手"}</span>${m.text}`;
    if (m.well) {
      const well = document.createElement("div");
      well.className = "well";
      well.textContent = m.well;
      div.append(well);
    }
    if (m.link) {
      const a = document.createElement("a");
      a.className = "msg-link";
      a.href = m.link.href;
      a.textContent = m.link.label;
      div.append(document.createElement("br"), a);
    }
    box.append(div);
  }
  return section(
    "c-session", "会话消息 🔆", "脱敏摘要 + 可跳转核对 + 六态尾注；无审批按钮（红线）",
    "session-message",
    panel(box, note("路径只显示相对路径；结论必附深链；脱敏规则见规格 §3")),
  );
}

function demoNotification({ fixtures }) {
  const list = document.createElement("div");
  list.className = "demo-grid";
  for (const n of fixtures.notifications) {
    const item = document.createElement("div");
    item.className = `notice-item${n.unread ? " unread" : ""}`;
    item.innerHTML = `
      <span class="notice-kind">${n.kind}</span>
      <p style="margin:.35rem 0 .2rem;font-weight:${n.unread ? "600" : "400"}">${n.title}</p>
      <p style="margin:0 0 .35rem;color:var(--mfd-color-text-secondary);font-size:var(--mfd-font-size-sm)">${n.body}</p>
      <div class="demo-row" style="justify-content:space-between">
        <span class="notice-time">${n.time}</span>
        <span class="demo-row" style="gap:var(--mfd-space-2)"></span>
      </div>`;
    item.querySelector(".demo-row:last-child").append(button("查看任务", "link"), button("标记已读", "secondary", "btn-compact"));
    if (!n.unread) item.querySelector(".btn-secondary").disabled = true;
    list.append(item);
  }
  return section("c-notification", "通知", "未读左条纹 + kind 胶囊；跳转即读；角标只计待介入项", "notification", list);
}

function demoNavigation() {
  const nav = document.createElement("nav");
  nav.className = "panel";
  nav.innerHTML = `
    <div class="demo-row" style="gap:var(--mfd-space-1)">
      <a class="btn btn-link" style="color:var(--mfd-color-text-inverse);background:var(--mfd-color-bg-inverse-hover)">概览</a>
      <a class="btn btn-link" style="color:var(--mfd-color-text-inverse)">任务 <span class="badge badge-danger" style="border-radius:var(--mfd-radius-full)">3</span></a>
      <a class="btn btn-link" style="color:var(--mfd-color-text-inverse)">媒体</a>
    </div>`;
  return section(
    "c-navigation", "导航", "本站侧栏即 navigation 规格实例（inverse 深底 + 角标 + 图标化中宽档 + 移动底部）",
    "navigation",
    nav,
    note("调整窗口宽度观察三档：≥64rem 文字侧栏 / 48–64rem 图标列 / <48rem 顶栏形态"),
  );
}

function demoStates() {
  const STATES = [
    ["loading", "◌", "正在加载任务列表…", "保留导航与骨架，避免跳动", null],
    ["empty", "▢", "暂无正式媒体记录", "完成待确认任务后即可建立。", "查看待确认任务"],
    ["error", "{!}", "任务列表加载失败", "task 列表在获取阶段失败：连接超时。下一步：稍后重试。", "重试"],
    ["offline", "⌁", "无法连接 MediaFlow 服务", "正在显示 5 分钟前的数据；需服务端确认的操作已禁用。", "重试"],
    ["conflict", "⚖", "计划与已存在文件冲突", "inbox/dune2-cn.mkv 与目标指纹相同。下一步：修正规则或目标。", "查看冲突详情"],
    ["partial", "◐", "整理部分完成", "3 项已完成，1 项未完成：NFO 写入失败。下一步：只重试未完成步骤。", "重试未完成步骤"],
    ["success", "✓", "整理完成", "文件操作、NFO 与媒体记录均完成。", "查看结果"],
  ];
  const grid = document.createElement("div");
  grid.className = "demo-grid";
  for (const [state, glyph, title, body, action] of STATES) {
    const box = document.createElement("div");
    box.className = "state-box";
    box.dataset.state = state;
    box.innerHTML = `
      <span class="state-icon${state === "loading" ? " spin" : ""}" aria-hidden="true">${glyph}</span>
      <span class="state-title">${title}</span>
      <span class="state-body">${body}</span>`;
    if (action) {
      const actions = document.createElement("div");
      actions.className = "state-actions";
      actions.append(button(action, "primary", "btn-compact"));
      box.append(actions);
    }
    grid.append(box);
  }
  return section("c-states", "六态容器 🔆", "状态→展示重点→唯一主操作；stale 共存；暗色下深 tint 底", "state-container", grid);
}

export async function renderComponentsView({ fixtures }) {
  const frag = document.createDocumentFragment();
  const h1 = document.createElement("h1");
  h1.className = "page-title";
  h1.textContent = "组件规格";
  const desc = document.createElement("p");
  desc.className = "page-desc";
  desc.textContent = "12 件组件 demo × 状态矩阵；右上角切换主题，全部 demo 双主题可查；🔆 = 主题敏感组件（暗色表现见规格 §5）";
  frag.append(h1, desc);
  frag.append(
    demoButton(),
    demoForm(),
    demoTable({ fixtures }),
    demoMediaWall({ fixtures }),
    demoBadge(),
    demoTimeline(),
    demoPlan({ fixtures }),
    demoApproval(),
    demoSession({ fixtures }),
    demoNotification({ fixtures }),
    demoNavigation(),
    demoStates(),
  );
  return frag;
}
