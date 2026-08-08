<script setup lang="ts">
/** 使用原生表单语义编辑有界 organization target，并强制先执行无副作用 preflight。 */
import ErrorSummary from "../../components/ErrorSummary.vue";
import { useOrganizationTargets } from "./useOrganizationTargets";

const props = defineProps<{
  feature: ReturnType<typeof useOrganizationTargets>;
  mode: "create" | "update";
}>();

async function submit(): Promise<void> {
  if (props.mode === "create") await props.feature.save();
  else await props.feature.update();
}
</script>

<template>
  <section class="form-card organization-target-form" :aria-labelledby="`${mode}-organization-target-heading`">
    <h2 :id="`${mode}-organization-target-heading`">{{ mode === "create" ? "添加整理目标" : "更新整理目标" }}</h2>
    <p class="field-help">只配置 deployment root ID 与根内相对目录；预检不会创建目录或保存配置。</p>
    <ErrorSummary
      :message="feature.formError.value?.message"
      :field="feature.formError.value?.field"
      :focus-key="feature.errorOccurrence.value"
      heading="整理目标尚未保存"
    />
    <p v-if="feature.conflictLatest.value" id="organization-conflict" tabindex="-1" role="status">
      最新服务端版本：{{ feature.conflictLatest.value.config_version }}。当前非敏感草稿仍保留，请重新预检。
    </p>
    <form @submit.prevent="submit">
      <label for="target-display-name">显示名</label>
      <input id="target-display-name" v-model="feature.form.displayName" required maxlength="120">

      <label for="target-kind">媒体类型</label>
      <select id="target-kind" v-model="feature.form.kind">
        <option value="movie">电影</option>
        <option value="series">剧集</option>
        <option value="generic-video">受限通用视频</option>
      </select>

      <label for="root-id">可写能力根</label>
      <select id="root-id" v-model="feature.form.rootId" required>
        <option value="" disabled>请选择 read-write 根</option>
        <option v-for="root in feature.writableRoots.value" :key="root.id" :value="root.id">{{ root.label }}（{{ root.id }}）</option>
      </select>

      <label for="relative-path">根内相对目录</label>
      <input id="relative-path" v-model="feature.form.relativePath" required maxlength="4096" autocomplete="off">

      <fieldset>
        <legend>固定文件操作</legend>
        <label><input v-model="feature.form.operation" type="radio" value="copy">复制（保留来源）</label>
        <label><input v-model="feature.form.operation" type="radio" value="move">移动（需要来源写权限）</label>
        <label><input v-model="feature.form.operation" type="radio" value="hardlink">硬链接（必须同文件系统）</label>
      </fieldset>

      <label for="naming-pattern">固定命名模式</label>
      <select id="naming-pattern" v-model="feature.form.namingPattern">
        <option value="movie">电影标题与年份</option>
        <option value="series">剧集季/集层级</option>
        <option value="generic-numbered">通用视频分组编号</option>
      </select>

      <label for="nfo-policy">NFO 策略</label>
      <select id="nfo-policy" v-model="feature.form.nfoPolicy">
        <option value="preserve-only">只保留已有 NFO</option>
        <option value="generate-missing">仅生成缺失 NFO</option>
      </select>

      <label class="check-field"><input v-model="feature.form.automatic" type="checkbox">允许命中有界规则的低风险计划自动执行</label>
      <label class="check-field"><input v-model="feature.form.enabled" type="checkbox">启用目标</label>

      <fieldset class="organization-rules">
        <legend>有界规则</legend>
        <p class="field-help">规则只按媒体类型、可选收件目录 ID 与显式标签匹配，不支持脚本或正则表达式。</p>
        <fieldset v-for="(rule, index) in feature.form.rules" :key="index" class="organization-rule">
          <legend>规则 {{ index + 1 }}</legend>
          <label :for="`rule-kind-${index}`">媒体类型</label>
          <select :id="`rule-kind-${index}`" v-model="rule.media_kind">
            <option value="movie">电影</option><option value="series">剧集</option><option value="generic-video">通用视频</option>
          </select>
          <label :for="`rule-inbox-${index}`">收件目录 ID（可选）</label>
          <input :id="`rule-inbox-${index}`" v-model="rule.inbox_directory_id" maxlength="36" autocomplete="off">
          <label :for="`rule-tag-${index}`">显式标签（可选）</label>
          <input :id="`rule-tag-${index}`" v-model="rule.explicit_tag" maxlength="64" autocomplete="off">
          <label class="check-field"><input v-model="rule.enabled" type="checkbox">启用此规则</label>
          <button type="button" @click="feature.removeRule(index)">移除规则</button>
        </fieldset>
        <button type="button" @click="feature.addRule">添加规则</button>
      </fieldset>

      <div class="detail-actions">
        <button type="button" :disabled="!feature.canWrite.value" @click="feature.preflight">检查目标</button>
        <button class="primary-action" type="submit" :disabled="!feature.canWrite.value || !feature.preflightMatches.value">
          {{ mode === "create" ? "保存目标" : "保存更新" }}
        </button>
      </div>
      <p v-if="feature.preflightResult.value" role="status">
        预检通过：{{ feature.preflightResult.value.root_id }}/<span class="safe-path">{{ feature.preflightResult.value.relative_path }}</span>
        · {{ feature.preflightResult.value.same_filesystem_hint === true ? "当前同文件系统" : feature.preflightResult.value.same_filesystem_hint === false ? "当前跨文件系统" : "文件系统关系未知" }}
      </p>
    </form>
  </section>
</template>
