<script setup lang="ts">
/**
 * `tasks/:id` 路由的实例边界。
 *
 * 路由参数变化（同组件 A→B、历史前进/后退）时按 id 键强制重建详情视图，使数据加载、
 * 写操作、返回上下文与 SSE 订阅始终绑定当前任务对象；旧实例的迟到响应随卸载丢弃。
 */
import { useRoute } from "vue-router";
import ProcessingTaskDetailView from "./ProcessingTaskDetailView.vue";

const route = useRoute();
</script>

<template>
  <ProcessingTaskDetailView :key="String(route.params.id)" />
</template>
