// MediaFlow Web lint 门禁：TypeScript strict-type-checked + Vue 推荐规则。
// 原则（任务工程规范）：禁 any/as（白名单例外在下方 rules 显式登记）；类型安全优先于便利。
import pluginVue from 'eslint-plugin-vue';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**', 'coverage/**', 'playwright-report/**', 'src/generated/**'] },
  ...tseslint.configs.strictTypeChecked,
  ...pluginVue.configs['flat/recommended'],
  {
    files: ['**/*.vue'],
    languageOptions: {
      parserOptions: { parser: tseslint.parser },
    },
  },
  {
    languageOptions: {
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: ['.vue'],
      },
    },
    rules: {
      // 禁显式 any；确需逃逸时用 unknown + 收窄，并在决策日志登记。
      '@typescript-eslint/no-explicit-any': 'error',
      // 断言只允许 as const 与必要的 DOM/生成类型桥接；对象字面量断言一律禁止。
      '@typescript-eslint/consistent-type-assertions': [
        'error',
        { assertionStyle: 'as', objectLiteralTypeAssertions: 'never' },
      ],

    },
  },
  {
    // 工具脚本（.js/.mjs）无 tsconfig project，走基础解析，不做 type-checked 检查。
    files: ['eslint.config.js', 'scripts/**/*.mjs'],
    extends: [tseslint.configs.disableTypeChecked],
  },
  {
    // Vue 模板格式规则（属性行宽、标签换行等）是格式化器职责，不在正确性 lint 范围；
    // 待引入格式化工具时统一接管，避免本门禁产生千级格式噪音。
    files: ['**/*.vue'],
    rules: {
      'vue/max-attributes-per-line': 'off',
      'vue/singleline-html-element-content-newline': 'off',
      'vue/html-self-closing': 'off',
      'vue/html-indent': 'off',
      'vue/html-closing-bracket-newline': 'off',
      'vue/first-attribute-linebreak': 'off',
      'vue/multiline-html-element-content-newline': 'off',
    },
  },
  {
    // 测试与 E2E：放行测试 mock 的固有模式（async stub 无 await、方法引用、fixture 断言、
    // void 箭头简写）；这些是测试构造惯例而非类型安全缺陷，修复不产生价值。
    files: ['tests/**', 'e2e/**', '*.config.ts', 'scripts/**'],
    rules: {
      '@typescript-eslint/no-non-null-assertion': 'off',
      '@typescript-eslint/require-await': 'off',
      '@typescript-eslint/unbound-method': 'off',
      '@typescript-eslint/consistent-type-assertions': 'off',
      '@typescript-eslint/no-confusing-void-expression': 'off',
    },
  },
);
