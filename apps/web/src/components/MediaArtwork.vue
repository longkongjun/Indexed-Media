<script setup lang="ts">
/**
 * 展示正式媒体条目的本地封面登记状态，供列表和详情页复用。
 *
 * 组件只根据父级提供的本地不透明引用显示可用或缺失状态，不请求、拼接或展示第三方图片 URL；可见状态完全来自 props。
 */
import type { MediaItemPage } from "@mediaflow/api-client-ts";

type ArtworkRef = NonNullable<MediaItemPage["items"][number]["artwork_ref"]>;
/** 传入媒体标题和 Core 已登记的本地封面状态，用于生成可访问的展示文案。 */
defineProps<{ artwork: ArtworkRef | null; title: string }>();
</script>

<template>
  <div class="media-artwork" role="img" :aria-label="artwork?.state === 'available' ? `${title} 的本地封面已登记` : `${title} 暂无本地封面`">
    <span aria-hidden="true">{{ artwork?.state === 'available' ? '▣' : '□' }}</span>
    <small>{{ artwork?.state === 'available' ? '本地封面已登记' : '暂无本地封面' }}</small>
  </div>
</template>
