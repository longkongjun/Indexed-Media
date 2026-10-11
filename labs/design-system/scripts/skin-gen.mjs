#!/usr/bin/env node
// 皮肤生成器（M3 tonal palette 机制，零依赖）：
//   seed 定义（各 palette 的 OKLCH hue/chroma 锚）→ tone 阶（L 感知亮度）→ 语义槽位映射（M3 scheme 精神）
//   → 输出 tokens/skins/<name>.json（颜色 token 全集 light/dark + 风格参数覆盖）。
// 生成后由 check-contrast.mjs（全皮肤 × 双主题）驱动微调；文件入库后不再是构建期依赖。
// 用法：node scripts/skin-gen.mjs

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const outDir = join(here, "..", "tokens", "skins");

/* ---------- OKLCH → sRGB（OKLab 转换，W3C CSS Color 4 同算法） ---------- */

function oklchToRgb(L, C, Hdeg) {
  const h = (Hdeg * Math.PI) / 180;
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.291485548 * b;
  const l = l_ ** 3, m = m_ ** 3, s = s_ ** 3;
  let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
  let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
  let bb = -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s;
  const gamma = (v) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);
  const enc = (v) => Math.round(Math.min(1, Math.max(0, gamma(v))) * 255);
  return [enc(r), enc(g), enc(bb)];
}

/** 色域钳制：保持 L 与 H，收缩 C 直至入 sRGB 色域（M3 gamut mapping 精神）。 */
function oklchToHex(L, C, H) {
  let c = C;
  for (let i = 0; i < 24; i += 1) {
    const [r, g, b] = oklchToRgb(L, c, H);
    if (r !== 0 && r !== 255 && g !== 0 && g !== 255 && b !== 0 && b !== 255) return hex(r, g, b);
    c *= 0.94;
  }
  const [r, g, b] = oklchToRgb(L, c, H);
  return hex(r, g, b);
}

const hex = (r, g, b) => "#" + [r, g, b].map((v) => v.toString(16).padStart(2, "0")).join("");

/* ---------- tone 阶：M3 tone 值 → OKLCH L（感知亮度近似映射） ---------- */

// tone（M3 语义 0-100）→ OKLCH L：锚点线性插值（M3 tone 为连续标度）
const TONE_ANCHORS = [[0, 0], [10, 0.108], [20, 0.194], [30, 0.288], [40, 0.384], [50, 0.482], [60, 0.585], [70, 0.692], [80, 0.782], [90, 0.872], [100, 1.0]];

const toneL = (tone) => {
  if (!Number.isFinite(tone) || tone < 0 || tone > 100) throw new Error(`tone 越界: ${tone}`);
  for (let i = 1; i < TONE_ANCHORS.length; i += 1) {
    const [t1, l1] = TONE_ANCHORS[i];
    const [t0, l0] = TONE_ANCHORS[i - 1];
    if (tone <= t1) return l0 + ((l1 - l0) * (tone - t0)) / (t1 - t0);
  }
  return 1.0;
};
const t = (palette, tone) => oklchToHex(toneL(tone), palette.C, palette.H);
// 低饱和变体（容器底等：色度减半）
const tSoft = (palette, tone) => oklchToHex(toneL(tone), palette.C * 0.5, palette.H);

/* ---------- 皮肤 seed ---------- */

const SKINS = {
  aurora: {
    label: "极光",
    desc: "青蓝主色 + 冷中性；锐利密度（半径略小）",
    style: { radius: { sm: "0.375rem", md: "0.5rem", lg: "0.625rem", xl: "0.875rem", "2xl": "1.125rem" } },
    p: { primary: { H: 240, C: 0.125 }, neutral: { H: 250, C: 0.008 }, nv: { H: 248, C: 0.012 },
         success: { H: 152, C: 0.11 }, warning: { H: 80, C: 0.115 }, danger: { H: 25, C: 0.135 }, info: { H: 232, C: 0.115 } },
  },
  sepia: {
    label: "胶片",
    desc: "暖棕主色 + 暖中性；柔和手感（半径更大）",
    style: { radius: { sm: "0.5rem", md: "0.625rem", lg: "0.875rem", xl: "1.125rem", "2xl": "1.375rem" } },
    p: { primary: { H: 55, C: 0.105 }, neutral: { H: 65, C: 0.01 }, nv: { H: 62, C: 0.014 },
         success: { H: 142, C: 0.1 }, warning: { H: 72, C: 0.115 }, danger: { H: 28, C: 0.13 }, info: { H: 238, C: 0.1 } },
  },
};

/* ---------- 语义槽位映射（M3 scheme 规则的精神，槽位为 MFD 语义） ---------- */

function scheme(p, style) {
  const P = p.primary, N = p.neutral, V = p.nv;
  return {
    color: {
      bg: {
        canvas: { light: t(N, 96), dark: t(N, 8), desc: "" },
        surface: { light: t(N, 99), dark: t(V, 12), desc: "" },
        "surface-raised": { light: t(N, 100), dark: t(V, 18), desc: "" },
        "surface-sunken": { light: t(N, 94), dark: t(N, 4), desc: "" },
        hover: { light: `rgba(16 37 29 / 0.06)`, dark: `rgba(230 237 232 / 0.06)`, desc: "" },
        selected: { light: tSoft(P, 92), dark: tSoft(P, 22), desc: "" },
        inverse: { light: tSoft(P, 12), dark: t(N, 4), desc: "" },
        "inverse-hover": { light: tSoft(P, 22), dark: t(V, 14), desc: "" },
        scrim: { light: "rgba(6 12 9 / 0.55)", dark: "rgba(0 0 0 / 0.65)", desc: "" },
        highlight: { light: "rgba(255 255 255 / 0.6)", dark: "rgba(255 255 255 / 0.04)", desc: "" },
      },
      text: {
        primary: { light: t(N, 10), dark: t(N, 94), desc: "" },
        secondary: { light: t(V, 35), dark: t(V, 76), desc: "" },
        muted: { light: t(V, 38), dark: t(V, 70), desc: "" },
        inverse: { light: t(N, 96), dark: t(N, 96), desc: "" },
        "inverse-muted": { light: t(V, 76), dark: t(V, 72), desc: "" },
        "on-action": { light: "#ffffff", dark: t(N, 12), desc: "" },
        "on-danger": { light: "#ffffff", dark: "#200a0a", desc: "" },
        link: { light: t(P, 42), dark: t(P, 78), desc: "" },
      },
      border: {
        subtle: { light: t(V, 88), dark: t(V, 16), desc: "" },
        default: { light: t(V, 80), dark: t(V, 20), desc: "" },
        strong: { light: t(V, 58), dark: t(V, 50), desc: "" },
        focus: { light: t(P, 48), dark: t(P, 80), desc: "" },
        inverse: { light: tSoft(P, 26), dark: t(V, 18), desc: "" },
      },
      action: {
        primary: { light: t(P, 48), dark: t(P, 80), desc: "" },
        "primary-hover": { light: t(P, 44), dark: t(P, 84), desc: "" },
        "primary-active": { light: t(P, 42), dark: t(P, 76), desc: "" },
        "secondary-bg": { light: t(N, 100), dark: t(V, 16), desc: "" },
        danger: { light: t(p.danger, 40), dark: t(p.danger, 76), desc: "" },
        "danger-hover": { light: t(p.danger, 36), dark: t(p.danger, 82), desc: "" },
        gradient: { light: `linear-gradient(180deg, ${t(P, 50)} 0%, ${t(P, 48)} 100%)`, dark: `linear-gradient(180deg, ${t(P, 82)} 0%, ${t(P, 80)} 100%)`, desc: "" },
        "gradient-text": { light: `linear-gradient(90deg, ${t(P, 56)} 0%, ${t(P, 44)} 100%)`, dark: `linear-gradient(90deg, ${t(P, 84)} 0%, ${t(P, 78)} 100%)`, desc: "" },
      },
      state: {
        success: { light: t(p.success, 38), dark: t(p.success, 78), desc: "" },
        "success-bg": { light: tSoft(p.success, 92), dark: tSoft(p.success, 22), desc: "" },
        warning: { light: t(p.warning, 40), dark: t(p.warning, 80), desc: "" },
        "warning-bg": { light: tSoft(p.warning, 94), dark: tSoft(p.warning, 26), desc: "" },
        "warning-border": { light: t(p.warning, 52), dark: t(p.warning, 56), desc: "" },
        danger: { light: t(p.danger, 40), dark: t(p.danger, 76), desc: "" },
        "danger-bg": { light: tSoft(p.danger, 95), dark: tSoft(p.danger, 24), desc: "" },
        "danger-border": { light: t(p.danger, 52), dark: t(p.danger, 62), desc: "" },
        info: { light: t(p.info, 40), dark: t(p.info, 78), desc: "" },
        "info-bg": { light: tSoft(p.info, 93), dark: tSoft(p.info, 22), desc: "" },
        "info-border": { light: t(p.info, 80), dark: t(p.info, 40), desc: "" },
      },
      media: {
        placeholder: { light: t(V, 88), dark: t(V, 16), desc: "" },
        "badge-bg": { light: "rgba(12 18 15 / 0.85)", dark: "rgba(0 0 0 / 0.85)", desc: "" },
        "badge-text": { light: "#ffffff", dark: "#ffffff", desc: "" },
        "scrim-bottom": { light: "linear-gradient(to top, rgba(11 18 14 / 0.72), transparent)", dark: "linear-gradient(to top, rgba(0 0 0 / 0.8), transparent)", desc: "" },
      },
      chart: {
        "1": { light: t(P, 48), dark: t(P, 80), desc: "" },
        "2": { light: t(p.info, 46), dark: t(p.info, 78), desc: "" },
        "3": { light: t(p.warning, 48), dark: t(p.warning, 78), desc: "" },
        "4": { light: t(p.danger, 48), dark: t(p.danger, 74), desc: "" },
        "5": { light: t(V, 48), dark: t(V, 76), desc: "" },
        grid: { light: t(V, 88), dark: t(V, 16), desc: "" },
        axis: { light: t(V, 35), dark: t(V, 76), desc: "" },
      },
    },
    style,
  };
}

mkdirSync(outDir, { recursive: true });
for (const [name, def] of Object.entries(SKINS)) {
  const doc = {
    name,
    label: def.label,
    desc: def.desc,
    generator: "scripts/skin-gen.mjs（M3 tonal palette 机制：OKLCH seed → tone 阶 → 语义槽位）",
    ...scheme(def.p, def.style),
  };
  const file = join(outDir, `${name}.json`);
  writeFileSync(file, JSON.stringify(doc, null, 2) + "\n", "utf8");
  console.log(`skin ${name} (${def.label}) -> ${file}`);
}
console.log("注意：生成值须过 scripts/check-contrast.mjs（全皮肤 × 双主题）后方可定稿；微调直接改 JSON 或调 seed 重跑。");
