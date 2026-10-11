// 本地 fixtures：组件 demo 数据（沿用 labs/web-prototype 本地模拟数据先例，不连任何服务）。

let tokensCache = null;

export async function fetchTokens() {
  if (tokensCache) return tokensCache;
  const response = await fetch("assets/tokens.json");
  tokensCache = await response.json();
  return tokensCache;
}

export const fixtures = {
  tasks: [
    { id: "task-1042", stage: "文件操作", status: "failed", statusTone: "danger", statusGlyph: "▲", statusLabel: "失败", summary: "目标路径已存在（conflict）" },
    { id: "task-1043", stage: "识别", status: "waiting", statusTone: "warning", statusGlyph: "◐", statusLabel: "待确认", summary: "候选 3 个，最高置信度 0.92" },
    { id: "task-1044", stage: "NFO", status: "partial", statusTone: "warning", statusGlyph: "◐", statusLabel: "部分成功", summary: "已完成 2/3" },
    { id: "task-1045", stage: "完成", status: "done", statusTone: "success", statusGlyph: "◆", statusLabel: "完成", summary: "整理、NFO 与记录均完成" },
    { id: "task-1046", stage: "计划", status: "running", statusTone: "info", statusGlyph: "◌", statusLabel: "运行中", summary: "重新计算计划 v8" },
  ],
  media: [
    { title: "沙丘 2", year: "2024", kind: "电影", registered: true },
    { title: "地球脉动 第三季", year: "2023", kind: "剧集", registered: true },
    { title: "家庭聚会 2026 春节", year: "2026", kind: "通用视频", registered: false },
    { title: "未命名项目", year: "", kind: "通用视频", registered: false },
  ],
  plan: {
    version: 7,
    task: "task-1043",
    groups: [
      {
        target: "媒体库/电影/沙丘 2 (2024)/",
        ops: [
          { type: "move", label: "移动", tone: "info", glyph: "→", from: "inbox/dune2.mkv", to: "媒体库/电影/沙丘 2 (2024)/沙丘 2 (2024).mkv" },
          { type: "nfo", label: "NFO", tone: "success", glyph: "▤", from: "（生成）", to: "媒体库/电影/沙丘 2 (2024)/movie.nfo" },
        ],
      },
      {
        target: "隔离区/冲突/",
        ops: [
          { type: "quarantine", label: "隔离", tone: "warning", glyph: "⚑", from: "inbox/dune2-cn.mkv", to: "隔离区/冲突/dune2-cn.mkv", conflict: "与已存在文件指纹相同" },
        ],
      },
    ],
  },
  events: [
    { sequence: 123, type: "task.waiting_approval", label: "等待审批", taskId: "task-1043" },
    { sequence: 122, type: "task.updated", label: "任务更新", taskId: "task-1046" },
    { sequence: 121, type: "media.created", label: "媒体建立", taskId: "task-1045" },
  ],
  notifications: [
    { kind: "待审确认", title: "task-1043 的整理计划等待审批", body: "计划 v7：2 项移动，1 项隔离。", time: "3 分钟前", unread: true },
    { kind: "待审确认", title: "task-1042 在文件操作阶段失败", body: "目标路径已存在（conflict）。下一步：修正规则或目标后重新计算计划。", time: "1 小时前", unread: false },
  ],
  session: [
    { role: "user", text: "上次那个纪录片叫什么？" },
    { role: "ai", text: "最匹配的是《地球脉动 第三季》(2023)，已建立正式记录。", well: "剧集/地球脉动 第三季 (2023)/", link: { label: "媒体详情 →", href: "#/patterns" } },
    { role: "ai", text: "task-1043 的计划已就绪：2 项移动、1 项隔离。需要你在 Web 审批页核对完整影响后批准。", link: { label: "任务 #1043 →", href: "#/patterns" } },
  ],
};
