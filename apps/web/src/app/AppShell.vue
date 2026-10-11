<script setup lang="ts">
/**
 * 已认证的响应式应用外壳，提供主导航、账户身份信息、外观偏好和退出登录功能。
 *
 * 组件没有 props、发出事件或暴露的实例方法。它读取会话 store，在本地切换账户面板；退出登录会清除会话状态
 * 并导航到登录页。外观偏好经 app/theme 控制器落 data-theme/data-skin 属性与本地记忆。
 */
import { computed, inject, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import AiSessionPanel from "./AiSessionPanel.vue";
import { identityClientKey } from "./client";
import { useMediaFlowClient } from "../components/injectedClient";
import { useSessionStore } from "./session";
import { useJellyfin } from "../features/jellyfin/useJellyfin";
import { applySkinChoice, applyThemeChoice, readSkinChoice, readThemeChoice, type SkinChoice, type ThemeChoice } from "./theme";

const router = useRouter();
const client = inject(identityClientKey, null);
const session = useSessionStore();
const accountOpen = ref(false);

const jellyfin = useJellyfin(useMediaFlowClient());
const jellyfinHealthLabel = computed(() => {
  const health = jellyfin.integration.value?.health;
  if (health === "healthy") return "连接正常";
  if (health === "unauthorized") return "凭据被拒绝";
  if (health === "unavailable") return "无法访问";
  if (health === "degraded") return "待复检";
  return "未配置";
});

const themeChoice = ref<ThemeChoice>(readThemeChoice());
const skinChoice = ref<SkinChoice>(readSkinChoice());
const themeChoices: ReadonlyArray<{ value: ThemeChoice; label: string }> = [
  { value: "system", label: "跟随系统" },
  { value: "light", label: "浅色" },
  { value: "dark", label: "暗色" },
];
const skinChoices: ReadonlyArray<{ value: SkinChoice; label: string }> = [
  { value: "evergreen", label: "常青" },
  { value: "aurora", label: "极光" },
  { value: "sepia", label: "胶片" },
];

function chooseTheme(value: ThemeChoice): void {
  themeChoice.value = value;
  applyThemeChoice(value);
}

function chooseSkin(value: SkinChoice): void {
  skinChoice.value = value;
  applySkinChoice(value);
}

async function logout(): Promise<void> {
  await session.logout(router, client?.deleteSession.bind(client));
}

watch(accountOpen, (open) => {
  if (open) void jellyfin.load();
});
onMounted(() => {
  void jellyfin.load();
});
</script>

<template>
  <div class="app-shell">
    <aside class="sidebar">
      <a class="brand" href="#main-content">MediaFlow</a>
      <nav data-desktop-nav aria-label="主导航">
        <RouterLink to="/tasks">任务中心</RouterLink>
        <RouterLink to="/media">媒体</RouterLink>
        <RouterLink to="/inbox-directories">收件目录</RouterLink>
        <RouterLink to="/organization/targets">整理目标</RouterLink>
        <RouterLink to="/downloads">下载任务</RouterLink>
        <RouterLink to="/connections/downloaders">下载器连接</RouterLink>
        <RouterLink to="/automation/sources">来源自动化</RouterLink>
        <RouterLink to="/capability-events">AI 能力活动</RouterLink>
        <RouterLink to="/notifications">通知中心</RouterLink>
        <button data-account-toggle type="button" aria-controls="account-panel" :aria-expanded="accountOpen" @click="accountOpen = !accountOpen">账户与系统</button>
      </nav>
      <AiSessionPanel />
    </aside>
    <div id="main-content" class="content" tabindex="-1">
      <section v-if="accountOpen" id="account-panel" data-account-panel class="account-panel" aria-labelledby="account-heading">
        <h2 id="account-heading">账户与系统</h2>
        <p v-if="session.account">当前管理员：{{ session.account.administrator_name }}</p>
        <RouterLink to="/automation/sources">管理来源自动化</RouterLink>
        <RouterLink to="/capability-events">AI 能力活动</RouterLink>
        <p v-if="jellyfin.integration.value" data-jellyfin-status>
          Jellyfin：{{ jellyfinHealthLabel }}<template v-if="jellyfin.integration.value.server_version">
            （{{ jellyfin.integration.value.server_version }}）</template>
          <a
            v-if="jellyfin.integration.value.base_url"
            :href="jellyfin.integration.value.base_url"
            target="_blank"
            rel="noopener noreferrer"
            data-jellyfin-home-link
          >打开 Jellyfin 首页</a>
        </p>
        <fieldset class="appearance-field" data-appearance>
          <legend>外观</legend>
          <div class="appearance-group" role="radiogroup" aria-label="主题">
            <label v-for="choice in themeChoices" :key="choice.value" class="appearance-option">
              <input
                type="radio"
                name="theme-choice"
                :value="choice.value"
                :checked="themeChoice === choice.value"
                @change="chooseTheme(choice.value)"
              />
              <span>{{ choice.label }}</span>
            </label>
          </div>
          <div class="appearance-group" role="radiogroup" aria-label="皮肤">
            <label v-for="choice in skinChoices" :key="choice.value" class="appearance-option">
              <input
                type="radio"
                name="skin-choice"
                :value="choice.value"
                :checked="skinChoice === choice.value"
                @change="chooseSkin(choice.value)"
              />
              <span>{{ choice.label }}</span>
            </label>
          </div>
        </fieldset>
        <button type="button" @click="logout">退出登录</button>
      </section>
      <RouterView />
    </div>
    <nav data-mobile-nav class="mobile-nav" aria-label="移动主导航">
      <RouterLink to="/tasks">任务</RouterLink>
      <RouterLink to="/media">媒体</RouterLink>
      <RouterLink to="/inbox-directories">收件目录</RouterLink>
      <RouterLink to="/organization/targets">整理</RouterLink>
      <RouterLink to="/downloads">下载</RouterLink>
      <RouterLink to="/automation/sources">自动化</RouterLink>
      <button type="button" aria-controls="account-panel" :aria-expanded="accountOpen" @click="accountOpen = !accountOpen">更多</button>
    </nav>
  </div>
</template>

<style scoped>
.app-shell { min-height: 100vh; background: var(--mfd-color-bg-canvas); color: var(--mfd-color-text-primary); }
.sidebar { position: fixed; inset: 0 auto 0 0; width: 15rem; box-sizing: border-box; padding: 1.5rem; background: var(--mfd-color-bg-inverse); color: var(--mfd-color-text-inverse); }
.brand { display: block; margin-bottom: 2rem; color: var(--mfd-color-text-inverse); font-size: 1.25rem; font-weight: 750; text-decoration: none; }
nav { display: flex; gap: .5rem; }
.sidebar nav { flex-direction: column; }
nav a, nav button { min-height: 2.75rem; box-sizing: border-box; padding: .75rem 1rem; border: 0; border-radius: .6rem; background: transparent; color: inherit; font: inherit; text-align: left; text-decoration: none; }
nav a:hover, nav button:hover { background: var(--mfd-color-bg-inverse-hover); }
nav a:focus-visible, nav button:focus-visible { outline: var(--mfd-stroke-focus-ring) solid var(--mfd-color-border-focus); outline-offset: 1px; background: var(--mfd-color-bg-inverse-hover); }
nav a.router-link-active { box-shadow: inset var(--mfd-stroke-focus-ring) 0 0 var(--mfd-color-border-focus); background: var(--mfd-color-bg-inverse-hover); font-weight: var(--mfd-font-weight-semibold); }
.content { min-height: 100vh; margin-left: 15rem; box-sizing: border-box; padding: 2rem; }
.account-panel { margin-bottom: 1.5rem; padding: 1rem; border: 1px solid var(--mfd-color-border-default); border-radius: var(--mfd-radius-lg); background: var(--mfd-color-bg-surface); }
.account-panel h2 { margin-top: 0; }
.account-panel button { min-height: 2.75rem; padding: .65rem 1rem; border: 1px solid var(--mfd-color-action-danger); border-radius: .5rem; background: var(--mfd-color-action-secondary-bg); color: var(--mfd-color-action-danger); }
.account-panel button:focus-visible { outline: 3px solid var(--mfd-color-border-focus); outline-offset: 2px; }
.appearance-field { margin: 0; padding: 0; border: 0; display: grid; gap: .5rem; }
.appearance-field legend { padding: 0; font-weight: 700; }
.appearance-group { display: flex; flex-wrap: wrap; gap: .5rem; }
.appearance-option { display: inline-flex; align-items: center; gap: .4rem; min-height: 2.75rem; box-sizing: border-box; padding: .3rem .7rem; border: 1px solid var(--mfd-color-border-default); border-radius: .5rem; background: var(--mfd-color-action-secondary-bg); cursor: pointer; }
.appearance-option:has(input:checked) { border-color: var(--mfd-color-action-primary); background: var(--mfd-color-bg-selected); font-weight: 650; }
.appearance-option input { min-height: 0; width: auto; margin: 0; accent-color: var(--mfd-color-action-primary); }
.appearance-option:focus-within { outline: 3px solid var(--mfd-color-border-focus); outline-offset: 2px; }
.mobile-nav { display: none; }
@media (max-width: 48rem) {
  .sidebar { position: static; width: auto; height: 4rem; padding: 1rem; }
  .sidebar nav { display: none; }
  .brand { margin: 0; }
  .content { min-height: calc(100vh - 8.5rem); margin-left: 0; padding: 1.25rem 1rem 5.5rem; }
  .mobile-nav { position: fixed; z-index: 5; inset: auto 0 0; display: grid; grid-template-columns: repeat(7, 1fr); padding: .5rem max(.5rem, env(safe-area-inset-right)) max(.5rem, env(safe-area-inset-bottom)) max(.5rem, env(safe-area-inset-left)); background: var(--mfd-color-bg-inverse); color: var(--mfd-color-text-inverse); }
  .mobile-nav a, .mobile-nav button { min-width: 0; padding: .5rem .2rem; font-size: .75rem; line-height: 1.2; text-align: center; white-space: nowrap; }
}
</style>
