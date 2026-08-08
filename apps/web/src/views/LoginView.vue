<script setup lang="ts">
/**
 * 管理员登录页，提供可访问的验证摘要并有意清除密码。
 *
 * 组件没有 props、发出事件或暴露的实例方法。提交会验证本地字段，在等待身份请求前清除密码，使失败获得焦点，
 * 成功登录则通过会话状态重定向。
 */
import { computed, nextTick, onBeforeUnmount, reactive, ref } from "vue";
import { useIdentity } from "../features/identity/useIdentity";

type LoginField = "login-administrator-name" | "login-password";

const administratorName = ref("");
const password = ref("");
const summary = ref<HTMLElement | null>(null);
const fieldErrors = reactive<Record<LoginField, string | null>>({
  "login-administrator-name": null,
  "login-password": null,
});
const fieldErrorOrder = ref<LoginField[]>([]);
const identity = useIdentity();
let disposed = false;

const summaryItems = computed(() => {
  const items: Array<{ target?: LoginField; message: string }> = [];
  const orderedTargets = [
    ...fieldErrorOrder.value,
    ...(Object.keys(fieldErrors) as LoginField[]).filter((target) => !fieldErrorOrder.value.includes(target)),
  ];
  for (const target of orderedTargets) {
    const message = fieldErrors[target];
    if (message) items.push({ target, message });
  }
  if (identity.error.value) {
    const targets = identity.errorTargets.value.filter((target): target is LoginField => target in fieldErrors);
    if (targets.length) targets.forEach((target) => items.push({ target, message: identity.error.value! }));
    else items.push({ message: identity.error.value });
  }
  return items;
});

function clearPassword(): void {
  password.value = "";
}

onBeforeUnmount(() => {
  disposed = true;
  clearPassword();
});

function describedBy(field: LoginField): string | undefined {
  const ids: string[] = [];
  if (fieldErrors[field]) ids.push(`${field}-error`);
  if (identity.errorTargets.value.includes(field)) ids.push("login-error");
  return ids.length ? ids.join(" ") : undefined;
}

function invalid(field: LoginField): boolean {
  return Boolean(fieldErrors[field] || identity.errorTargets.value.includes(field));
}

async function submit(): Promise<void> {
  if (identity.submitting.value) return;
  identity.clearError();
  fieldErrors["login-administrator-name"] = administratorName.value ? null : "请输入管理员名称。";
  fieldErrors["login-password"] = password.value ? null : "请输入密码。";
  if (Object.values(fieldErrors).some(Boolean)) {
    const originalErrors = (Object.keys(fieldErrors) as LoginField[]).filter((field) => fieldErrors[field]);
    const clearedFields: LoginField[] = [];
    if (!fieldErrors["login-password"]) {
      fieldErrors["login-password"] = "出于安全考虑，密码已清空，请重新输入。";
      clearedFields.push("login-password");
    }
    fieldErrorOrder.value = [...originalErrors, ...clearedFields];
    clearPassword();
    await nextTick();
    summary.value?.focus();
    return;
  }
  fieldErrorOrder.value = [];
  const request = { administrator_name: administratorName.value, password: password.value };
  clearPassword();
  const succeeded = await identity.login(request);
  if (!succeeded && !disposed) {
    await nextTick();
    summary.value?.focus();
  }
}
</script>

<template>
  <main class="identity-page">
    <section class="identity-card" aria-labelledby="login-title">
      <p class="eyebrow">MediaFlow</p>
      <h1 id="login-title">管理员登录</h1>
      <p>登录凭据只发送到此 MediaFlow 实例。</p>
      <div v-if="summaryItems.length" id="login-error" ref="summary" data-error-summary class="error-summary" tabindex="-1" role="alert" aria-live="assertive">
        <h2>无法登录</h2>
        <ul>
          <li v-for="(item, index) in summaryItems" :key="`${item.target ?? 'general'}-${index}`">
            <a v-if="item.target" :href="`#${item.target}`">{{ item.message }}</a>
            <span v-else>{{ item.message }}</span>
          </li>
        </ul>
      </div>
      <form novalidate @submit.prevent="submit">
        <label for="login-administrator-name">管理员名称</label>
        <input id="login-administrator-name" v-model="administratorName" name="administrator_name" autocomplete="username" required :aria-invalid="invalid('login-administrator-name')" :aria-describedby="describedBy('login-administrator-name')" />
        <p v-if="fieldErrors['login-administrator-name']" id="login-administrator-name-error" class="field-error">{{ fieldErrors["login-administrator-name"] }}</p>

        <label for="login-password">密码</label>
        <input id="login-password" v-model="password" name="password" type="password" autocomplete="current-password" required :aria-invalid="invalid('login-password')" :aria-describedby="describedBy('login-password')" @keydown.enter.prevent="submit" />
        <p v-if="fieldErrors['login-password']" id="login-password-error" class="field-error">{{ fieldErrors["login-password"] }}</p>

        <button type="submit" :disabled="identity.submitting.value">{{ identity.submitting.value ? "正在登录…" : "登录" }}</button>
      </form>
    </section>
  </main>
</template>

<style scoped>
.identity-page { display: grid; min-height: 100vh; place-items: center; box-sizing: border-box; padding: 1.25rem; background: #e8f0eb; color: #10251d; }
.identity-card { width: min(100%, 30rem); box-sizing: border-box; padding: clamp(1.5rem, 5vw, 2.5rem); border-radius: 1rem; background: white; box-shadow: 0 1rem 3rem #10251d1a; }
.eyebrow { color: #28684d; font-weight: 700; }
form { display: grid; gap: .65rem; margin-top: 1.5rem; }
label { margin-top: .5rem; font-weight: 700; }
input { min-height: 2.75rem; box-sizing: border-box; padding: .65rem .8rem; border: 1px solid #63756d; border-radius: .45rem; font: inherit; }
input:focus-visible, button:focus-visible, a:focus-visible, .error-summary:focus { outline: 3px solid #0b7a50; outline-offset: 2px; }
button { min-height: 2.75rem; margin-top: .75rem; border: 0; border-radius: .5rem; background: #176b49; color: white; font: inherit; font-weight: 700; }
button:disabled { opacity: .65; }
.field-error { margin: 0; color: #8b2020; font-weight: 650; }
.error-summary { padding: 1rem; border-inline-start: .3rem solid #a82727; background: #fff1f1; }
.error-summary h2 { margin-top: 0; font-size: 1rem; }
</style>
