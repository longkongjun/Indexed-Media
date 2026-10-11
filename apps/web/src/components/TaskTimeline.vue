<script setup lang="ts">
/**
 * 任务时间线（task-timeline 规格 v1.0，detail 变体）：阶段推进、当前阶段与每阶段真实结果。
 *
 * 阶段链：发现→识别→计划→文件操作→NFO→完成（"发现"由任务存在即文件已发现承载，无独立服务端阶段）。
 * 节点符双编码（●完成 ◉进行中 ○未开始 ⚠需处理 ◐部分成功）；手风琴同时只展开一个阶段，
 * 问题节点默认展开直达原因。恢复动作由页面级操作区承载（避免双处主操作），本组件只陈述事实。
 */
import type { ProcessingTask, ProcessingTaskOrganization } from "@mediaflow/api-client-ts";
import { computed, ref } from "vue";
import { ORGANIZATION_JOURNAL_KIND_LABELS, ORGANIZATION_JOURNAL_STATUS_LABELS, PROCESSING_STAGE_LABELS } from "./dictionaries";

/** 服务端权威的处理任务投影与可选组织投影（计划/journal/本地结果）。 */
const props = defineProps<{ task: ProcessingTask; organization: ProcessingTaskOrganization | null }>();

type NodeState = "done" | "current" | "pending" | "attention";

interface StageNode {
  key: string;
  label: string;
  state: NodeState;
  shape: string;
  statusText: string;
  facts: string[];
}

const STAGE_ORDER: ProcessingTask["stage"][] = ["identification", "planning", "file-operation", "nfo", "completion"];
const stagePassed = (stage: ProcessingTask["stage"]) => STAGE_ORDER.indexOf(props.task.stage) > STAGE_ORDER.indexOf(stage);

const plan = computed(() => props.organization?.plan ?? null);
const localResult = computed(() => props.organization?.local_result ?? null);

const journalsFacts = (): string[] => props.organization?.journals.map(
  (journal) => `${ORGANIZATION_JOURNAL_KIND_LABELS[journal.kind]}：${journal.destination.root_id}/${journal.destination.relative_path} · ${ORGANIZATION_JOURNAL_STATUS_LABELS[journal.status]}`,
) ?? [];

const nodes = computed<StageNode[]>(() => {
  const task = props.task;
  const result: StageNode[] = [
    {
      key: "discovery",
      label: "发现",
      state: "done",
      shape: "●",
      statusText: "已完成",
      facts: [`发现文件：${task.relative_path}`],
    },
  ];

  // 识别
  const identifying = task.stage === "identification";
  const reviewNeeded = task.status === "waiting-confirmation" || task.decision_checkpoint === "manual-decision-pending" || task.decision_checkpoint === "rematch-pending";
  result.push({
    key: "identification",
    label: PROCESSING_STAGE_LABELS.identification,
    state: reviewNeeded ? "attention" : identifying ? "current" : stagePassed("identification") ? "done" : "pending",
    shape: reviewNeeded ? "⚠" : identifying ? "◉" : stagePassed("identification") ? "●" : "○",
    statusText: reviewNeeded ? "需要人工处理" : identifying ? "进行中" : stagePassed("identification") ? "已完成" : "未开始",
    facts: reviewNeeded ? ["该任务的识别结论需要人工确认（候选或转通用视频）。"] : [],
  });

  // 计划
  const planPaused = plan.value?.authorization === "paused";
  const planning = task.stage === "planning";
  const planReady = plan.value !== null;
  const planState: NodeState = planPaused ? "attention" : planning ? "current" : planReady && (stagePassed("planning") || localResult.value !== null) ? "done" : "pending";
  result.push({
    key: "planning",
    label: PROCESSING_STAGE_LABELS.planning,
    state: planState,
    shape: planPaused ? "⚠" : planning ? "◉" : planState === "done" ? "●" : "○",
    statusText: planPaused ? "等待审批" : planning ? "进行中" : planState === "done" ? "已完成" : "未开始",
    facts: plan.value
      ? [`计划 v${String(plan.value.version)}：${plan.value.source.root_id}/${plan.value.source.relative_path} → ${plan.value.destination.root_id}/${plan.value.destination.relative_path}`]
      : [],
  });

  // 文件操作
  const journals = props.organization?.journals ?? [];
  const journalAttention = task.checkpoint === "file-operation-manual-review" || journals.some((journal) => journal.status === "manual-review");
  const fileOperating = task.stage === "file-operation";
  const fileDone = journals.length > 0 && journals.every((journal) => journal.status === "verified" || journal.status === "compensated") && stagePassed("file-operation");
  const fileState: NodeState = journalAttention ? "attention" : fileOperating ? "current" : fileDone ? "done" : "pending";
  result.push({
    key: "file-operation",
    label: PROCESSING_STAGE_LABELS["file-operation"],
    state: fileState,
    shape: journalAttention ? "⚠" : fileOperating ? "◉" : fileDone ? "●" : "○",
    statusText: journalAttention ? "待人工复核" : fileOperating ? "执行中" : fileDone ? "已验证" : "未开始",
    facts: journalsFacts(),
  });

  // NFO
  const nfoStatus = localResult.value?.nfo_status;
  const nfoAttention = nfoStatus === "failed" || task.checkpoint === "nfo-failed";
  const nfoActive = task.stage === "nfo" || task.checkpoint === "nfo-pending";
  const nfoDone = nfoStatus === "generated" || nfoStatus === "preserved" || task.checkpoint === "nfo-verified" || (stagePassed("nfo") && nfoStatus !== undefined);
  const nfoState: NodeState = nfoAttention ? "attention" : nfoActive ? "current" : nfoDone ? "done" : "pending";
  result.push({
    key: "nfo",
    label: PROCESSING_STAGE_LABELS.nfo,
    state: nfoState,
    shape: nfoAttention ? "⚠" : nfoActive ? "◉" : nfoDone ? "●" : "○",
    statusText: nfoAttention ? "失败" : nfoActive ? "进行中" : nfoDone ? (nfoStatus === "preserved" ? "已保留" : "已生成") : nfoStatus === "not-requested" ? "未请求（不适用）" : "未开始",
    facts: nfoAttention ? ["NFO 写入失败；重试只继续该步骤，不重复已验证的文件操作。"] : [],
  });

  // 完成
  const partial = task.status === "partial-success" || localResult.value?.status === "partial-success";
  const complete = task.status === "completed" || localResult.value?.status === "completed";
  const completionActive = task.stage === "completion";
  const completionState: NodeState = partial ? "attention" : complete ? "done" : completionActive ? "current" : "pending";
  result.push({
    key: "completion",
    label: PROCESSING_STAGE_LABELS.completion,
    state: completionState,
    shape: partial ? "◐" : complete ? "●" : completionActive ? "◉" : "○",
    statusText: partial ? "部分成功" : complete ? "已完成" : completionActive ? "进行中" : "未开始",
    facts: localResult.value
      ? [
          `本地结果：${localResult.value.status}`,
          ...(localResult.value.remaining_actions.length > 0 ? [`未完成步骤：${localResult.value.remaining_actions.join("、")}`] : []),
        ]
      : [],
  });

  const currentKeyByStage: Record<ProcessingTask["stage"], string> = {
    identification: "identification",
    planning: "planning",
    "file-operation": "file-operation",
    nfo: "nfo",
    completion: "completion",
  };
  const currentNode = result.find((node) => node.key === currentKeyByStage[props.task.stage]);
  if (currentNode) {
    currentNode.facts = [
      `持久检查点：${props.task.checkpoint}`,
      ...(props.task.decision_checkpoint ? [`人工决定：${props.task.decision_checkpoint}`] : []),
      ...currentNode.facts,
    ];
  }
  return result;
});

const expanded = ref<string | null>(null);
const problemNode = computed(() => nodes.value.find((node) => node.state === "attention" && node.facts.length > 0) ?? null);
expanded.value = problemNode.value?.key ?? null;

function toggle(node: StageNode): void {
  expanded.value = expanded.value === node.key ? null : node.key;
}

function onNodeKeydown(event: KeyboardEvent, index: number): void {
  const delta = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : event.key === "ArrowUp" || event.key === "ArrowLeft" ? -1 : 0;
  if (delta === 0) return;
  event.preventDefault();
  const next = index + delta;
  const buttons = document.querySelectorAll<HTMLButtonElement>("[data-timeline-node]");
  const target = next >= 0 && next < buttons.length ? buttons[next] : null;
  target?.focus();
}
</script>
<template>
  <ol class="task-timeline" aria-label="处理时间线" data-task-timeline>
    <li v-for="(node, index) in nodes" :key="node.key" class="task-timeline__node" :data-timeline-state="node.state" :aria-current="node.state === 'current' ? 'step' : undefined">
      <button
        type="button"
        class="task-timeline__toggle"
        :data-timeline-node="node.key"
        :aria-expanded="expanded === node.key"
        @click="toggle(node)"
        @keydown="onNodeKeydown($event, index)"
      >
        <span class="task-timeline__shape" :data-tone="node.state === 'attention' ? 'warning' : node.state === 'done' ? 'success' : node.state === 'current' ? 'info' : 'neutral'" aria-hidden="true">{{ node.shape }}</span>
        <span class="task-timeline__label">{{ node.label }}</span>
        <span class="task-timeline__status">{{ node.statusText }}</span>
      </button>
      <div v-if="expanded === node.key && node.facts.length > 0" class="task-timeline__facts">
        <p v-for="fact in node.facts" :key="fact">{{ fact }}</p>
      </div>
    </li>
  </ol>
</template>
