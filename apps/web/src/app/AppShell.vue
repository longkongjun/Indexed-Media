<script setup lang="ts">
/**
 * 已认证的响应式应用外壳，提供主导航、账户身份信息和退出登录功能。
 *
 * 组件没有 props、发出事件或暴露的实例方法。它读取会话 store，在本地切换账户面板；退出登录会清除会话状态
 * 并导航到登录页。
 */
import { inject, ref } from "vue";
import { useRouter } from "vue-router";
import { identityClientKey } from "./client";
import { useSessionStore } from "./session";

const router = useRouter();
const client = inject(identityClientKey, null);
const session = useSessionStore();
const accountOpen = ref(false);

async function logout(): Promise<void> {
  await session.logout(router, client?.deleteSession.bind(client));
}
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
        <button data-account-toggle type="button" aria-controls="account-panel" :aria-expanded="accountOpen" @click="accountOpen = !accountOpen">账户与系统</button>
      </nav>
    </aside>
    <div id="main-content" class="content" tabindex="-1">
      <section v-if="accountOpen" id="account-panel" data-account-panel class="account-panel" aria-labelledby="account-heading">
        <h2 id="account-heading">账户与系统</h2>
        <p v-if="session.account">当前管理员：{{ session.account.administrator_name }}</p>
        <RouterLink to="/automation/sources">管理来源自动化</RouterLink>
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
.app-shell { min-height: 100vh; background: #f4f7f5; color: #12231c; }
.sidebar { position: fixed; inset: 0 auto 0 0; width: 15rem; box-sizing: border-box; padding: 1.5rem; background: #10251d; color: white; }
.brand { display: block; margin-bottom: 2rem; color: white; font-size: 1.25rem; font-weight: 750; text-decoration: none; }
nav { display: flex; gap: .5rem; }
.sidebar nav { flex-direction: column; }
nav a, nav button { min-height: 2.75rem; box-sizing: border-box; padding: .75rem 1rem; border: 0; border-radius: .6rem; background: transparent; color: inherit; font: inherit; text-align: left; text-decoration: none; }
nav a:hover, nav a:focus-visible, nav button:hover, nav button:focus-visible, nav a.router-link-active { outline: 3px solid #7de0af; outline-offset: 1px; background: #1c4937; }
.content { min-height: 100vh; margin-left: 15rem; box-sizing: border-box; padding: 2rem; }
.account-panel { margin-bottom: 1.5rem; padding: 1rem; border: 1px solid #b9c9c1; border-radius: .75rem; background: white; }
.account-panel h2 { margin-top: 0; }
.account-panel button { min-height: 2.75rem; padding: .65rem 1rem; border: 1px solid #8b2a2a; border-radius: .5rem; background: white; color: #7b2020; }
.account-panel button:focus-visible { outline: 3px solid #0b7a50; outline-offset: 2px; }
.mobile-nav { display: none; }
@media (max-width: 48rem) {
  .sidebar { position: static; width: auto; height: 4rem; padding: 1rem; }
  .sidebar nav { display: none; }
  .brand { margin: 0; }
  .content { min-height: calc(100vh - 8.5rem); margin-left: 0; padding: 1.25rem 1rem 5.5rem; }
  .mobile-nav { position: fixed; z-index: 5; inset: auto 0 0; display: grid; grid-template-columns: repeat(7, 1fr); padding: .5rem max(.5rem, env(safe-area-inset-right)) max(.5rem, env(safe-area-inset-bottom)) max(.5rem, env(safe-area-inset-left)); background: #10251d; color: white; }
  .mobile-nav a, .mobile-nav button { min-width: 0; padding: .5rem .2rem; font-size: .75rem; line-height: 1.2; text-align: center; white-space: nowrap; }
}
</style>
