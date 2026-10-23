import { existsSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * 包契约测试：主题 token 与冻结值源对账、组件变量完整性、样式选择器作用域、库依赖边界。
 *
 * 值源：labs/design-system/tokens/tokens.json v2.0.0（evergreen 基线）。本测试只读该文件，
 * 保证 src/styles/theme.css 手维护子集不随源漂移；组件样式与主题均不依赖 labs 运行时。
 */
const pkgRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(pkgRoot, "..", "..");
const tokensJson = JSON.parse(readFileSync(resolve(repoRoot, "labs/design-system/tokens/tokens.json"), "utf8")) as {
  semantic: unknown;
};

type SemanticLeaf = { value?: string | undefined; light?: string | undefined; dark?: string | undefined };

/** 把 semantic 树按 “层级名以 - 连接” 展平为 CSS 变量名 → 叶子（value 或 light/dark）。 */
function flattenSemantic(node: unknown, path: string[], out: Map<string, SemanticLeaf>): void {
  if (!node || typeof node !== "object") return;
  const record = node as Record<string, unknown>;
  const hasValue = typeof record.value === "string";
  const hasTheme = typeof record.light === "string" && typeof record.dark === "string";
  if (hasValue || hasTheme) {
    out.set(path.join("-"), {
      value: typeof record.value === "string" ? record.value : undefined,
      light: typeof record.light === "string" ? record.light : undefined,
      dark: typeof record.dark === "string" ? record.dark : undefined,
    });
    return;
  }
  for (const [key, child] of Object.entries(record)) {
    if (key.startsWith("_")) continue;
    flattenSemantic(child, [...path, key], out);
  }
}

const semanticTokens = new Map<string, SemanticLeaf>();
flattenSemantic(tokensJson.semantic, [], semanticTokens);

const themeCss = readFileSync(resolve(pkgRoot, "src/styles/theme.css"), "utf8");
const buttonSfc = readFileSync(resolve(pkgRoot, "src/components/MfButton.vue"), "utf8");

/** 提取一段 CSS 中全部 --mfd-* 变量声明。 */
function declaredVars(css: string): Map<string, string> {
  const vars = new Map<string, string>();
  for (const match of css.matchAll(/(--mfd-[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
    const name = match[1];
    const value = match[2];
    if (name !== undefined && value !== undefined) vars.set(name, value.trim());
  }
  return vars;
}

/** 按选择器片段截取其平衡花括号块内容。 */
function blockAfter(css: string, selector: string): string {
  const start = css.indexOf(selector);
  expect(start, `theme.css 缺少选择器 ${selector}`).toBeGreaterThanOrEqual(0);
  const open = css.indexOf("{", start);
  let depth = 0;
  for (let i = open; i < css.length; i += 1) {
    if (css[i] === "{") depth += 1;
    if (css[i] === "}") {
      depth -= 1;
      if (depth === 0) return css.slice(open + 1, i);
    }
  }
  throw new Error(`theme.css 中 ${selector} 块未闭合`);
}

const rootVars = declaredVars(blockAfter(themeCss, ":root"));
const darkVars = declaredVars(blockAfter(themeCss, '[data-theme="dark"]'));
const schemeVars = declaredVars(blockAfter(blockAfter(themeCss, "@media (prefers-color-scheme: dark)"), ":root:not([data-theme])"));

describe("主题 token 与冻结值源对账", () => {
  it("浅色块每个变量等于 tokens.json 的 light/value", () => {
    expect(rootVars.size).toBeGreaterThan(0);
    for (const [name, cssValue] of rootVars) {
      const token = semanticTokens.get(name.replace(/^--mfd-/, ""));
      expect(token, `${name} 不在 tokens.json semantic 中`).toBeDefined();
      expect(cssValue, `${name} 浅色值与 tokens.json 不一致`).toBe(token?.light ?? token?.value);
    }
  });

  it("暗色块与系统暗色回退块等于 tokens.json 的 dark，且只含双主题 token", () => {
    for (const vars of [darkVars, schemeVars]) {
      for (const [name, cssValue] of vars) {
        const token = semanticTokens.get(name.replace(/^--mfd-/, ""));
        expect(token, `${name} 不在 tokens.json semantic 中`).toBeDefined();
        expect(token?.dark, `${name} 不是双主题 token，不应出现在暗色块`).toBeTypeOf("string");
        expect(cssValue, `${name} 暗色值与 tokens.json 不一致`).toBe(token?.dark);
      }
    }
  });

  it("组件样式引用的 --mfd-* 变量全部由主题入口定义", () => {
    const used = new Set(
      [...buttonSfc.matchAll(/var\((--mfd-[a-z0-9-]+)/g)].map((m) => m[1]).filter((name): name is string => name !== undefined),
    );
    expect(used.size).toBeGreaterThan(0);
    for (const name of used) {
      expect(rootVars.has(name), `组件使用了主题未定义的变量 ${name}`).toBe(true);
    }
  });
});

describe("样式选择器作用域", () => {
  /** 断言一段 CSS 的所有选择器都以 .mf-button 或主题入口选择器开头，不选中宿主其他元素。 */
  function expectScoped(css: string): void {
    const withoutComments = css.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const match of withoutComments.matchAll(/([^{}]+)\{/g)) {
      const raw = match[1];
      if (raw === undefined) continue;
      const selector = raw.trim();
      if (selector.startsWith("@")) continue;
      // :where()/:not() 等函数式伪类内的逗号不是选择器分隔符，先整体剥离再分组。
      const functional = selector.replace(/::?[a-z-]+\([^)]*\)/g, "");
      for (const part of functional.split(",")) {
        const trimmed = part.trim();
        if (trimmed.length === 0) continue;
        if (
          trimmed.startsWith(".mf-button") ||
          trimmed.startsWith(":root") ||
          trimmed.startsWith("[data-theme") ||
          trimmed.startsWith("[data-skin") ||
          trimmed === "to"
        ) {
          continue;
        }
        throw new Error(`选择器超出作用域: ${trimmed}`);
      }
    }
  }

  it("组件样式不污染宿主 button", () => {
    const styleMatch = buttonSfc.match(/<style[^>]*>([\s\S]*?)<\/style>/);
    expect(styleMatch).not.toBeNull();
    expectScoped(styleMatch?.[1] ?? "");
  });

  it("主题入口只定义 :root / data-theme / data-skin 层级", () => {
    expectScoped(themeCss);
  });
});

describe("库依赖边界", () => {
  it("package.json 只公开两个入口且 vue 为 peer 依赖", () => {
    const manifest = JSON.parse(readFileSync(resolve(pkgRoot, "package.json"), "utf8"));
    expect(Object.keys(manifest.exports).sort()).toEqual([".", "./style.css"]);
    expect(manifest.dependencies).toBeUndefined();
    expect(manifest.peerDependencies).toEqual({ vue: "^3.5.0" });
  });

  it("源码不引用业务包、路由、store 或网络 API", () => {
    const forbiddenImport = /from\s+["']((@mediaflow\/(api-client-ts|test-fixtures|web))|pinia|vue-router)["']/;
    const forbiddenNetwork = /\b(fetch\s*\(|XMLHttpRequest|axios\b)/;
    function scan(dir: string): void {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = resolve(dir, entry.name);
        if (entry.isDirectory()) {
          scan(full);
          continue;
        }
        if (!/\.(ts|vue|css)$/.test(entry.name)) continue;
        const content = readFileSync(full, "utf8");
        expect(content, `${full} 含被禁止的依赖导入`).not.toMatch(forbiddenImport);
        expect(content, `${full} 含网络调用`).not.toMatch(forbiddenNetwork);
      }
    }
    scan(resolve(pkgRoot, "src"));
  });
});
