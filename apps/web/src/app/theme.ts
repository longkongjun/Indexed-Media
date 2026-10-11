/**
 * 主题三态与皮肤选择控制器（设计系统 theme-switching 模式 §1-4 + skins 二维主题）。
 *
 * 三态模型：跟随系统（无 data-theme，交给 prefers-color-scheme）/ 手动浅色 / 手动暗色；
 * 记忆键 `mfd-theme`；皮肤键 `mfd-skin`（缺省 evergreen = 无属性）。index.html 的内联脚本
 * 已在首帧前恢复属性防闪，本模块只负责控件态对齐、切换与持久化。
 *
 * 切换过渡：窗口期内给 root 加 `theme-transition` 类（base.css 中仅对颜色类属性交叉淡入，
 * prefers-reduced-motion 时 CSS 侧自动归零）；窗口结束移除。
 */

export type ThemeChoice = "system" | "light" | "dark";
export type SkinChoice = "evergreen" | "aurora" | "sepia";

const THEME_KEY = "mfd-theme";
const SKIN_KEY = "mfd-skin";
const TRANSITION_CLASS = "theme-transition";
const TRANSITION_WINDOW_MS = 450;

const THEME_CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];
const SKIN_CHOICES: readonly SkinChoice[] = ["evergreen", "aurora", "sepia"];

function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    // 隐私模式等存储不可用：仅本次会话生效，不阻塞切换。
    return null;
  }
}

function writeStored(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 同上：静默降级。
  }
}

function isThemeChoice(value: string | null): value is ThemeChoice {
  return value !== null && (THEME_CHOICES as readonly string[]).includes(value);
}

function isSkinChoice(value: string | null): value is SkinChoice {
  return value !== null && (SKIN_CHOICES as readonly string[]).includes(value);
}

/** 读取持久化的主题选择；缺失或非法值归一为跟随系统。 */
export function readThemeChoice(): ThemeChoice {
  const stored = readStored(THEME_KEY);
  return isThemeChoice(stored) ? stored : "system";
}

/** 读取持久化的皮肤选择；缺失或非法值归一为常青基线。 */
export function readSkinChoice(): SkinChoice {
  const stored = readStored(SKIN_KEY);
  return isSkinChoice(stored) ? stored : "evergreen";
}

/** 打开切换过渡窗口（重复切换时刷新窗口）。 */
function openTransitionWindow(): void {
  const root = document.documentElement;
  root.classList.add(TRANSITION_CLASS);
  window.setTimeout(() => {
    root.classList.remove(TRANSITION_CLASS);
  }, TRANSITION_WINDOW_MS);
}

/** 应用主题选择：手动浅/暗写 data-theme，跟随系统删除属性交给媒体查询；并持久化。 */
export function applyThemeChoice(choice: ThemeChoice): void {
  const root = document.documentElement;
  if (choice === "light" || choice === "dark") root.dataset.theme = choice;
  else delete root.dataset.theme;
  writeStored(THEME_KEY, choice);
  openTransitionWindow();
  root.dispatchEvent(new CustomEvent("mfd-theme-change", { detail: { theme: choice } }));
}

/** 应用皮肤选择：非基线皮肤写 data-skin，基线删除属性；并持久化。 */
export function applySkinChoice(skin: SkinChoice): void {
  const root = document.documentElement;
  if (skin === "aurora" || skin === "sepia") root.dataset.skin = skin;
  else delete root.dataset.skin;
  writeStored(SKIN_KEY, skin);
  openTransitionWindow();
  root.dispatchEvent(new CustomEvent("mfd-skin-change", { detail: { skin } }));
}

/**
 * 启动时对齐：index.html 防闪脚本已恢复属性，这里归一非法存储值并触发一次持久化。
 * 不加过渡窗口（首帧不需要动画）。
 */
export function initTheme(): void {
  writeStored(THEME_KEY, readThemeChoice());
  writeStored(SKIN_KEY, readSkinChoice());
}
