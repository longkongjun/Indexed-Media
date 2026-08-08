<script setup lang="ts">
/**
 * 一次性管理员初始化页面，提供可访问验证并主动清除凭据。
 *
 * 组件没有 props、发出事件或暴露的实例方法。提交会在等待 Core 前清除引导密钥和密码，使失败获得焦点；
 * 成功初始化会使初始化状态失效并打开登录页。
 */
import { computed, nextTick, onBeforeUnmount, reactive, ref } from "vue";
import { useIdentity } from "../features/identity/useIdentity";

type SetupField = "bootstrap-secret" | "administrator-name" | "new-password";

const bootstrapSecret = ref("");
const administratorName = ref("");
const password = ref("");
const summary = ref<HTMLElement | null>(null);
const fieldErrors = reactive<Record<SetupField, string | null>>({
  "bootstrap-secret": null,
  "administrator-name": null,
  "new-password": null,
});
const fieldErrorOrder = ref<SetupField[]>([]);
const identity = useIdentity();
let disposed = false;

const summaryItems = computed(() => {
  const items: Array<{ target?: SetupField; message: string }> = [];
  const orderedTargets = [
    ...fieldErrorOrder.value,
    ...(Object.keys(fieldErrors) as SetupField[]).filter((target) => !fieldErrorOrder.value.includes(target)),
  ];
  for (const target of orderedTargets) {
    const message = fieldErrors[target];
    if (message) items.push({ target, message });
  }
  if (identity.error.value) {
    const targets = identity.errorTargets.value.filter((target): target is SetupField => target in fieldErrors);
    if (targets.length) targets.forEach((target) => items.push({ target, message: identity.error.value! }));
    else items.push({ message: identity.error.value });
  }
  return items;
});

function clearCredentials(): void {
  bootstrapSecret.value = "";
  password.value = "";
}

onBeforeUnmount(() => {
  disposed = true;
  clearCredentials();
});

function describedBy(field: SetupField): string | undefined {
  const ids: string[] = [];
  if (field === "new-password") ids.push("password-requirements");
  if (fieldErrors[field]) ids.push(`setup-${field}-error`);
  if (identity.errorTargets.value.includes(field)) ids.push("setup-error");
  return ids.length ? ids.join(" ") : undefined;
}

function invalid(field: SetupField): boolean {
  return Boolean(fieldErrors[field] || identity.errorTargets.value.includes(field));
}

async function submit(): Promise<void> {
  if (identity.submitting.value) return;
  identity.clearError();
  fieldErrors["bootstrap-secret"] = bootstrapSecret.value ? null : "请输入一次性引导密钥。";
  fieldErrors["administrator-name"] = administratorName.value ? null : "请输入管理员名称。";
  fieldErrors["new-password"] = password.value.length >= 12 ? null : "密码至少需要 12 个字符。";
  if (Object.values(fieldErrors).some(Boolean)) {
    const originalErrors = (Object.keys(fieldErrors) as SetupField[]).filter((field) => fieldErrors[field]);
    const clearedFields: SetupField[] = [];
    if (!fieldErrors["bootstrap-secret"]) {
      fieldErrors["bootstrap-secret"] = "出于安全考虑，引导密钥已清空，请重新输入。";
      clearedFields.push("bootstrap-secret");
    }
    if (!fieldErrors["new-password"]) {
      fieldErrors["new-password"] = "出于安全考虑，密码已清空，请重新输入。";
      clearedFields.push("new-password");
    }
    fieldErrorOrder.value = [...originalErrors, ...clearedFields];
    clearCredentials();
    await nextTick();
    summary.value?.focus();
    return;
  }
  fieldErrorOrder.value = [];
  const request = {
    bootstrap_secret: bootstrapSecret.value,
    administrator_name: administratorName.value,
    password: password.value,
  };
  clearCredentials();
  const succeeded = await identity.setup(request);
  if (!succeeded && !disposed) {
    await nextTick();
    summary.value?.focus();
  }
}
</script>

<template>
  <main class="identity-page">
    <section class="identity-card" aria-labelledby="setup-title">
      <p class="eyebrow">MediaFlow 本地设置</p>
      <h1 id="setup-title">创建管理员</h1>
      <p>创建此实例唯一的管理员。一次性引导密钥和密码不会保存在浏览器中。</p>
      <div v-if="summaryItems.length" id="setup-error" ref="summary" data-error-summary class="error-summary" tabindex="-1" role="alert" aria-live="assertive">
        <h2>无法完成初始化</h2>
        <ul>
          <li v-for="(item, index) in summaryItems" :key="`${item.target ?? 'general'}-${index}`">
            <a v-if="item.target" :href="`#${item.target}`">{{ item.message }}</a>
            <span v-else>{{ item.message }}</span>
          </li>
        </ul>
      </div>
      <form novalidate @submit.prevent="submit">
        <label for="bootstrap-secret">一次性引导密钥</label>
        <input id="bootstrap-secret" v-model="bootstrapSecret" name="bootstrap_secret" type="password" autocomplete="off" required :aria-invalid="invalid('bootstrap-secret')" :aria-describedby="describedBy('bootstrap-secret')" />
        <p v-if="fieldErrors['bootstrap-secret']" id="setup-bootstrap-secret-error" class="field-error">{{ fieldErrors["bootstrap-secret"] }}</p>

        <label for="administrator-name">管理员名称</label>
        <input id="administrator-name" v-model="administratorName" name="administrator_name" autocomplete="username" required :aria-invalid="invalid('administrator-name')" :aria-describedby="describedBy('administrator-name')" />
        <p v-if="fieldErrors['administrator-name']" id="setup-administrator-name-error" class="field-error">{{ fieldErrors["administrator-name"] }}</p>

        <label for="new-password">密码</label>
        <input id="new-password" v-model="password" name="password" type="password" autocomplete="new-password" minlength="12" required :aria-invalid="invalid('new-password')" :aria-describedby="describedBy('new-password')" @keydown.enter.prevent="submit" />
        <p id="password-requirements" class="hint">至少 12 个字符；建议使用多个不相关词语组成的长密码。</p>
        <p v-if="fieldErrors['new-password']" id="setup-new-password-error" class="field-error">{{ fieldErrors["new-password"] }}</p>

        <button type="submit" :disabled="identity.submitting.value">{{ identity.submitting.value ? "正在创建…" : "创建管理员" }}</button>
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
.hint { margin: 0; color: #465b52; font-size: .9rem; }
.field-error { margin: 0; color: #8b2020; font-weight: 650; }
.error-summary { padding: 1rem; border-inline-start: .3rem solid #a82727; background: #fff1f1; }
.error-summary h2 { margin-top: 0; font-size: 1rem; }
</style>
