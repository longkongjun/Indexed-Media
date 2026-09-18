# 注释与文档规范

## 1. 目的与适用范围

本规范回答两个问题：工程上何时需要注释、注释应解释什么；具体语言如何使用其原生文档语法表达这些信息。

规范适用于版本控制中的手写源码与受管可执行脚本，包括 `apps/**`、`packages/**`、`infra/**`、`labs/**` 和仓库根目录脚本。StudySpace 的 `workflow-packs/mediaflow/tools/` 不属于本仓库。以下内容不在直接编辑范围内：

- `archive/**`；
- 依赖目录、构建产物、缓存和锁文件；
- 标记为不可手改的生成输出。生成客户端的公开说明应修改其上游 OpenAPI 唯一来源，再通过已有生成流程同步。

这里的“公开 API”是指可被所属模块、包、组件或脚本边界之外调用的入口。语言中的可见性关键字只是判断依据之一；仅在私有模块内部使用的 `pub` 或 `export` 不会自动成为仓库级公开 API。

## 2. 规则强度

- **必须**：项目统一约束，代码审阅时必须满足。
- **应该**：默认推荐；存在明确上下文理由时可以偏离，并在评审中说明。
- **可以**：按可读性和工具能力选择。

本规范通过人工代码审阅维护，不为注释覆盖率新增 lint、解析器依赖、Just 或 CI 强制门禁。已有语言工具能够验证文档示例或语法时，可以继续使用，但其接线必须由对应实现 change 单独定义。

## 3. 工程级规则

### 3.1 何时必须写

以下位置必须有与声明相邻、使用语言原生格式的文档注释：

- 面向模块、包、组件或脚本外部使用者的公开 API；
- Vue 组件职责以及存在的 `defineProps`、`defineEmits`、`defineExpose` 公开契约；
- Shell 可执行入口、供其他脚本调用的函数和外部配置接口；
- `unsafe`、安全边界、持久化一致性、并发顺序、幂等或恢复约束等无法从类型和代码直接读出的关键契约。

复杂内部实现不要求机械覆盖，但遇到以下情况应该说明“为什么”：反直觉算法、重要不变量、平台差异、性能取舍、外部协议限制以及为兼容性保留的特殊处理。

### 3.2 应该解释什么

文档注释必须以使用者视角说明职责，并按适用情况解释：

- 输入和输出的业务语义，而不是重复类型；
- 失败、异常、错误码或取消语义；
- 文件、数据库、网络、日志、通知等可见副作用；
- 调用顺序、线程或协程安全、资源所有权与生命周期；
- 安全前置条件、数据边界和不可逆影响。

不适用的内容不需要创建空章节。简单访问器或显而易见的私有实现不应为了“有注释”而复述代码。

解释性正文必须使用中文。结构化标签、URL、命令、配置键、环境变量、协议字段和代码标识符保持原文。

### 3.3 禁止什么

- 禁止用同义句复述函数名、类型或下一行代码。
- 禁止保留与实现不一致的过期注释；行为变化必须同步更新相关文档。
- 禁止用注释掉的旧代码代替版本控制；待办事项应指向可追踪任务或说明解除条件。
- 禁止为了补注释顺手改变业务行为、对外协议或依赖方向。
- 禁止直接修改生成输出或任何 `archive/**` 内容。
- 禁止复制并维护第二份工程规则正文；索引、AGENTS 和 change 只链接本文件。

### 3.4 如何维护

- 新增或修改公开 API 时，同一变更必须更新其文档注释。
- 审阅者检查注释是否解释契约和原因，而不只检查是否存在。
- 发现存量缺口时可以在相关任务中补齐；范围较大时建立独立 change，不借普通业务任务扩大范围。
- 新语言或新模块初始化时，先复核其官方文档工具现状，再更新对应附录和模块局部规则。
- 语言附录与工程级规则冲突时，以工程级规则为准；语言附录只决定表达方式。

## 4. 语言级附录

### 4.1 Rust：rustdoc

- 条目文档使用 `///`，crate 或模块级说明使用 `//!`；首段给出简短职责摘要。
- 返回 `Result` 的公开 API 应使用 `# Errors` 说明失败条件；可能 panic 时应使用 `# Panics`。
- `unsafe` 公开 API 应使用 `# Safety` 说明调用者前置条件。
- Rust 示例代码应使用 rustdoc 围栏；按语义选择普通、`no_run`、`should_panic`、`compile_fail`、`ignore` 或非 Rust 文本标记，不能为了通过测试歪曲示例。
- 已初始化且包含可执行文档示例的 crate 应在对应实现 change 中运行 `cargo test --doc`；这不是本规范新增的仓库级门禁。

### 4.2 TypeScript、JavaScript 与 MJS：JSDoc

- 对外导出使用 `/** ... */`，首段说明职责。
- 参数、返回值、异常和泛型语义需要补充时，分别使用 `@param`、`@returns`、`@throws` 和 `@template`。
- TypeScript 类型系统已经表达的类型无需在标签中重复；注释重点解释业务含义、约束、失败和副作用。
- 仅供模块内部组合的导出不机械要求完整标签，但仍应在命名和结构无法表达意图时补充说明。

### 4.3 Vue：组件与公开契约

- 组件应在 `<script setup>` 的顶部说明职责、使用边界和重要状态来源。
- 存在 `defineProps`、`defineEmits` 或 `defineExpose` 时，使用紧邻 JSDoc 说明输入、事件或暴露能力的语义。
- 不在注释中重复模板结构；平台交互、无障碍或失败恢复约束应链接其权威设计或直接说明原因。

### 4.4 Shell：相邻 `#` 注释

- 可执行脚本在入口附近说明用途、参数、环境依赖、副作用和退出约定。
- 供其他脚本调用的函数在声明上方说明参数、输出与副作用。
- 外部可配置环境变量在首次读取处说明用途、默认值和敏感性；同一组退出码可以在入口或辅助函数附近集中说明，不要求为每个 `exit` 机械复制文字。
- Shell 没有统一文档生成语法，因此使用与目标相邻的 `#` 注释，并以可读性和可维护性为准。

### 4.5 Kotlin：KDoc

Kotlin 模块初始化后采用以下基线：

- 使用 `/** ... */`；首段为摘要，正文使用 Markdown。
- 使用 `@param`、`@return`、`@throws`/`@exception`、`@receiver`、`@property`、`@constructor`、`@sample`、`@see`、`@since` 和 `@suppress` 等 KDoc 标签表达适用语义。
- 主构造函数属性适合使用 `@property name`，主构造函数说明使用 `@constructor`。
- 代码符号链接使用 `[Foo.bar]`，不要复制 Javadoc 的 `{@link Foo#bar}` 写法。
- KDoc 结合 Javadoc 风格的块标签与 Markdown 行内标记，但不能简单等同于 Javadoc。

### 4.6 Swift：DocC

Swift 模块初始化后采用以下基线：

- 源码中的符号文档优先使用 `///`；摘要与详细说明之间保留空行。
- 参数可以使用一个嵌套的 `- Parameters:` 列表，也可以为每个参数使用 `- Parameter name:`；两种形式不按参数数量强制区分。
- 返回值和抛错条件分别使用 `- Returns:` 与 `- Throws:`。
- DocC 符号链接使用双反引号，例如 ```` ``Foo.bar()`` ````；提示信息可以使用 `- Note:`、`- Important:` 或 `- Warning:`。

### 4.7 Dart：dartdoc

Dart 或 Flutter 模块初始化后采用以下基线：

- 使用 `///`；首句作为独立摘要，后续说明另起段落。
- 参数、返回值和异常使用自然语言解释，不套用 Javadoc 的 `@param` 或 `@return`。
- 代码符号引用使用 `[identifier]`；普通代码片段仍可以使用 Markdown 反引号。
- 只有确实需要跨位置复用说明时才使用 dartdoc template/macro，避免为短文本增加间接层。

## 5. 评审清单

审阅新增或修改的注释时，至少确认：

1. 目标是否属于必须或应该说明的边界。
2. 注释是否解释职责、约束和原因，而不是复述代码。
3. 错误、副作用、安全和生命周期语义是否按实际情况覆盖。
4. 正文语言与结构化标识是否符合约定。
5. 是否使用对应语言的原生文档格式。
6. 注释是否随行为一起更新，且没有修改生成输出或归档内容。
7. 变更是否保持人工审阅边界，没有顺手引入自动门禁。

## 6. 语言资料

- [Rustdoc：How to write documentation](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)
- [Rustdoc：Documentation tests](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html)
- [JSDoc 官方标签参考](https://jsdoc.app/)
- [Vue：TypeScript with Composition API](https://vuejs.org/guide/typescript/composition-api.html)
- [Kotlin：KDoc](https://kotlinlang.org/docs/kotlin-doc.html)
- [Swift：Writing symbol documentation in your source files](https://developer.apple.com/documentation/xcode/writing-symbol-documentation-in-your-source-files)
- [Swift DocC：Formatting your documentation content](https://www.swift.org/documentation/docc/formatting-your-documentation-content)
- [Effective Dart：Documentation](https://dart.dev/effective-dart/documentation)

这些链接用于确认语言工具语法；MediaFlow 的工程边界和规则强度仍以本文件为准。
