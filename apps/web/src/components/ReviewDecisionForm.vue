<script setup lang="ts">
/**
 * 收集并校验人工识别决定，在父级允许的操作范围内生成提交请求。
 *
 * 组件不直接请求或持久化决定；可选操作、候选和初始标题来自父级，表单字段与错误焦点状态仅在组件内维护。
 */
import type { ReviewCandidatePage, ReviewCase, ReviewDecisionRequest } from "@mediaflow/api-client-ts";
import { computed, nextTick, ref, watch } from "vue";
import ErrorSummary from "./ErrorSummary.vue";
import ReviewCandidateList from "./ReviewCandidateList.vue";

/** 提供审核允许的决定、当前候选和初始提示，并在父级禁止写入时冻结表单。 */
const props = defineProps<{
  allowedActions: ReviewCase["allowed_actions"];
  candidates: ReviewCandidatePage["items"];
  disabled: boolean;
  initialTitle?: string | null;
}>();
/** 本地校验通过后向父级交付完整决定请求，由父级负责网络提交、冲突处理和刷新。 */
const emit = defineEmits<{ submit: [body: ReviewDecisionRequest] }>();
type Action = ReviewCase["allowed_actions"][number];

const kind = ref<Action>(props.allowedActions[0] ?? "rematch-with-hints");
const selectedCandidate = ref<string | null>(null);
const mediaType = ref<"movie" | "tv">("movie");
const title = ref(props.initialTitle ?? "");
const year = ref("");
const season = ref("");
const episodes = ref("");
const displayTitle = ref(props.initialTitle ?? "");
const groupHint = ref("");
const saveFeedback = ref(false);
const saveGroupingFeedback = ref(false);
const errorMessage = ref("");
const errorOccurrence = ref(0);
const form = ref<HTMLFormElement | null>(null);
const allowed = computed(() => new Set(props.allowedActions));

watch(() => props.allowedActions, (actions) => {
  if (!actions.includes(kind.value)) kind.value = actions[0] ?? "rematch-with-hints";
});

function integer(value: string, min: number, max: number): number | null | "invalid" {
  if (!value.trim()) return null;
  const parsed = Number(value);
  return Number.isInteger(parsed) && parsed >= min && parsed <= max ? parsed : "invalid";
}

async function invalid(message: string, selector?: string): Promise<void> {
  errorMessage.value = message;
  errorOccurrence.value += 1;
  await nextTick();
  await nextTick();
  const target = selector ? form.value?.querySelector<HTMLElement>(selector) : null;
  target?.focus();
}

async function submit(): Promise<void> {
  errorMessage.value = "";
  if (!allowed.value.has(kind.value)) return invalid("当前审核不允许该操作");
  if (kind.value === "select-provider-candidate") {
    const selected = props.candidates.find((item) => item.provider_id === selectedCandidate.value);
    if (!selected) return invalid("请选择一个候选", 'input[name="review-candidate"]');
    emit("submit", {
      kind: "select-provider-candidate",
      provider: selected.provider,
      media_type: selected.media_type,
      provider_id: selected.provider_id,
      save_feedback: saveFeedback.value,
    });
    return;
  }
  if (kind.value === "rematch-with-hints") {
    const normalizedTitle = title.value.trim();
    if (!normalizedTitle) return invalid("请输入重新匹配标题", "#review-title");
    const normalizedYear = integer(year.value, 1870, 2200);
    if (normalizedYear === "invalid") return invalid("年份必须在 1870 到 2200 之间", "#review-year");
    const normalizedSeason = mediaType.value === "tv" ? integer(season.value, 0, 999) : null;
    if (normalizedSeason === "invalid") return invalid("季号必须在 0 到 999 之间", "#review-season");
    const episodeValues = mediaType.value === "tv" && episodes.value.trim()
      ? episodes.value.split(",").map((part) => Number(part.trim()))
      : [];
    if (episodeValues.length > 32 || episodeValues.some((value) => !Number.isInteger(value) || value < 0 || value > 9999) || new Set(episodeValues).size !== episodeValues.length) {
      return invalid("集号应为 0 到 9999 的不重复数字，最多 32 个", "#review-episodes");
    }
    emit("submit", {
      kind: "rematch-with-hints",
      media_type: mediaType.value,
      title: normalizedTitle,
      year: normalizedYear,
      season: normalizedSeason,
      episodes: episodeValues,
      save_feedback: saveFeedback.value,
    });
    return;
  }
  const normalizedDisplayTitle = displayTitle.value.trim();
  if (!normalizedDisplayTitle) return invalid("请输入显示标题", "#generic-display-title");
  emit("submit", {
    kind: "select-generic-video",
    display_title: normalizedDisplayTitle,
    group_hint: groupHint.value.trim() || null,
    save_grouping_feedback: saveGroupingFeedback.value,
  });
}
</script>

<template>
  <form ref="form" class="review-decision-form" @submit.prevent="submit">
    <h2>提交人工决定</h2>
    <ErrorSummary v-if="errorMessage" :message="errorMessage" :focus-key="errorOccurrence" heading="请检查审核决定" />
    <label for="review-decision-kind">决定类型</label>
    <select id="review-decision-kind" v-model="kind" :disabled="disabled">
      <option v-if="allowed.has('select-provider-candidate')" value="select-provider-candidate">选择候选</option>
      <option v-if="allowed.has('rematch-with-hints')" value="rematch-with-hints">使用提示重新匹配</option>
      <option v-if="allowed.has('select-generic-video')" value="select-generic-video">作为通用视频</option>
    </select>

    <template v-if="kind === 'select-provider-candidate'">
      <ReviewCandidateList v-model="selectedCandidate" :candidates="candidates" :disabled="disabled" />
      <label class="check-field"><input id="review-save-feedback" v-model="saveFeedback" type="checkbox" :disabled="disabled"> 保存为精确反馈（默认关闭）</label>
    </template>

    <fieldset v-else-if="kind === 'rematch-with-hints'" :disabled="disabled">
      <legend>重新匹配提示</legend>
      <label for="review-media-type">媒体类型</label>
      <select id="review-media-type" v-model="mediaType"><option value="movie">电影</option><option value="tv">剧集</option></select>
      <label for="review-title">标题</label>
      <input id="review-title" v-model="title" maxlength="200" aria-describedby="review-title-help">
      <p id="review-title-help" class="field-help">必填；用于重新运行识别，不直接改文件。</p>
      <label for="review-year">年份（可选）</label>
      <input id="review-year" v-model="year" inputmode="numeric">
      <template v-if="mediaType === 'tv'">
        <label for="review-season">季号（可选）</label><input id="review-season" v-model="season" inputmode="numeric">
        <label for="review-episodes">集号（逗号分隔）</label><input id="review-episodes" v-model="episodes" placeholder="1,2">
      </template>
      <label class="check-field"><input id="review-save-feedback" v-model="saveFeedback" type="checkbox"> 保存为精确反馈（默认关闭）</label>
    </fieldset>

    <fieldset v-else :disabled="disabled">
      <legend>通用视频提示</legend>
      <label for="generic-display-title">显示标题</label>
      <input id="generic-display-title" v-model="displayTitle" maxlength="200">
      <label for="generic-group-hint">分组提示（可选）</label>
      <input id="generic-group-hint" v-model="groupHint" maxlength="200">
      <label class="check-field"><input v-model="saveGroupingFeedback" type="checkbox"> 保存分组反馈（默认关闭）</label>
      <p>此决定只记录通用视频意图；尚未执行规划或文件变更。</p>
    </fieldset>

    <button type="submit" :disabled="disabled || allowedActions.length === 0">确认提交</button>
  </form>
</template>
