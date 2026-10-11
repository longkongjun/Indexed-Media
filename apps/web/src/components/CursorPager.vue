<script setup lang="ts">
/**
 * 渲染游标分页控件，但不解释或保存不透明游标值。
 *
 * 输入决定哪些控件可见。它无数据地发出 `previous`，并携带提供的下一游标发出 `next`；组件不自行导航，
 * 也不暴露实例方法。可选 `shownCount` 提供位置提示（table-list 规格：『已显示 N 项』，游标对用户不透明）。
 */
/** `hasPreviousContext` 显示上下文返回操作；非空的 `nextCursor` 显示前进操作；`shownCount` 显示位置提示。 */
defineProps<{ hasPreviousContext?: boolean; nextCursor?: string | null; shownCount?: number }>();
/** 发出方向请求；`next` 携带通过 props 接收且保持不变的不透明游标。 */
defineEmits<{ previous: []; next: [cursor: string] }>();
</script>
<template>
  <nav class="cursor-pager" aria-label="结果分页">
    <button v-if="hasPreviousContext" type="button" @click="$emit('previous')">返回上一组结果</button>
    <p v-if="shownCount !== undefined" id="cursor-pager-position" class="cursor-position">已显示 {{ shownCount }} 项</p>
    <button v-if="nextCursor" data-next-cursor type="button" :aria-describedby="shownCount !== undefined ? 'cursor-pager-position' : undefined" @click="$emit('next', nextCursor)">继续查看更多</button>
  </nav>
</template>
