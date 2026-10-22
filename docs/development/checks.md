# 工程检查

命令定义以根 [justfile](../../justfile) 为准。`check` 是公共源码集合，`client-check` 是客户端完整集合，`check-all` 再包含镜像构建和 Compose 解析；三者的成功范围不同。平台缺项返回非零，不能把局部结果当成全量通过。TV、tvOS、Dart 和未初始化实验模块仍使用独立声明。

## 环境与工具

沿用 [Rust toolchain](../../rust-toolchain.toml)、[Node](../../.node-version)、[pnpm](../../package.json)、[Gradle wrapper](../../gradle/wrapper/gradle-wrapper.properties) 与[版本目录](../../gradle/libs.versions.toml)。Gradle 启动 JDK 与 daemon JVM 分开，后者由 `gradle/gradle-daemon-jvm.properties` 声明；fixture Java/Kotlin 输出均为 JVM 17。

just、cargo-deny、cargo-machete 和 SwiftLint 的固定版本见[工具清单](../../tools/selfcheck/tool-versions.env)。SwiftLint 可由 `bash tools/selfcheck/install-swiftlint.sh <新的本地目录>` 下载官方固定版本并验证 SHA-256，然后将该目录临时加入 PATH；不要求全局安装。iOS 使用完整 Xcode、XcodeGen 与已安装的模拟器，单独的 CommandLineTools 不够。默认目的地是 iPhone 17，可用 `MEDIAFLOW_IOS_DESTINATION` 指定已安装的同类 iOS Simulator。

源码部署自检需要 cargo、just、pnpm、node、python3、cargo-deny/machete；部署静态集合不要求 Docker。镜像构建/Compose 解析需要 Docker。Native、Android 编译仍需相应 SDK；运行交付镜像的 NAS 无需安装源码开发工具。

## 检查、测试发现与报告

| 范围 | 原子入口 | 测试发现与报告 | CI |
| --- | --- | --- | --- |
| Rust | `check-rust`、`test-rust-doc`、`check-rust-docs`、`check-rust-deps` | Cargo 自动发现 lib/bin/integration tests；doctest 独立执行。工具输出由 CI job 日志保留 | ci/rust、rust-deps |
| 契约/TS/夹具 | `check-contracts`、`check-api-client-ts`、`check-test-fixtures`、`check-ui-contract` | TS include 源/测试目录；各包现有 Vitest 套件；UI contract 类型与结构检查；生成声明逐字节比较，不覆写已提交文件 | ci/contracts |
| Web | `check-web` | `tests/` 的 Vitest（排除 e2e）；`e2e/` 的 Chromium desktop/mobile 两个项目；vue-tsc、ESLint、生产构建/体积。独立 `mediaflow-web-check.*/check.log`、dist、Playwright artifacts，退出时打印目录并保留 | ci/web |
| SDK | `client-check-kmp-sdk`、`client-check-kmp-jvm`、`client-docs-kmp-sdk` | ktlint、所有手写源集 Detekt；commonTest/jvmTest 经 jvmTest；macOS 的 iosSimulatorArm64Test。`packages/kmp-sdk/build/test-results/`、reports、Dokka HTML；Android/JVM/两个 Native 目标分别检查，Kover 只度量其 JVM 范围 | client-ci/kmp-sdk |
| Android | `client-check-android` | Kotlin/脚本 ktlint、lintDebug、assembleDebug、src/test 的 testDebugUnitTest；`apps/android/build/reports/`、test-results | client-ci/android |
| iOS | `client-lint-ios`、`client-check-ios` | SwiftLint 后生成工程，MediaFlowiOS scheme 的 MediaFlowiOSTests；独立 `mediaflow-ios-check.*/` 中完整日志、status.tsv 与 tests.xcresult | client-ci/ios |
| JVM 夹具服务 | `client-check-fixture-server` | ktlint + build；当前没有测试源，不把 NO-SOURCE 写成测试通过。报告在 `tools/client-fixture-server/build/reports/` | client-ci/fixture-server |
| 根构建脚本 | `client-check-build-scripts` | 根 Kotlin script ktlint；模块脚本由各模块 ktlint 覆盖 | client-ci/kmp-sdk |
| 基础设施 | `check-containers-static`、`check-compose-static`、`check-nas`；`check-containers`、`check-compose` 另含构建/解析 | 静态定义、镜像构建/inspect、Compose 解析分别保留；不代表真实 NAS 或运行时 smoke | ci/infra |
| 门禁传播回归 | `check-gate-regressions` | Python stdlib 隔离真实 recipe/脚本，覆盖原始工具失败、聚合、离线并存失败、缺环境、日志和安装包校验。替身不代签工具正常执行 | ci/contracts |

`check-android`、`check-ios`、`check-kmp-sdk` 转调真实入口；`check-mobile` 当前只聚合 Android/iOS。JVM 快速子集不声明 Native/iOS 已验证。SDK 没有 Android host test 变体；不能编造对应单测报告。

## 自检与诊断

`selfcheck` 逐项执行公共源码、依赖治理与部署静态检查；每次独立 `mediaflow-selfcheck.*/` 保存完整日志及 status.tsv。独立检查失败后仍运行后续项，失败返回 1，缺工具或离线 advisory 待验返回 2；两者并存时返回 1。离线模式不按日志关键字放行整个聚合，也不打印全量通过。

CI 通过同源原子入口运行，失败时保留 job 日志与适用报告。两套 workflow 是工程矩阵的不同部分，需按同一源码基线汇合真实 job 结果；静态解析或本地替身不算远端 CI 验收。真实 Core UI、设备、NAS、外部服务与发布验收继续走独立入口。

四处 TS 配置均开启 exactOptionalPropertyTypes；Vue Router5.2.0 的3行纯声明兼容修补由 `patches/` 与锁文件固定，其运行时 JS 和版本未变；上游修复后可单独移除补丁。SDK 的 LongParameterList、SwallowedException 与 TooGenericExceptionCaught 均采用默认检查，异常边界的局部 Suppress 只在其声明范围生效。cargo-deny 私有许可和重复版本等暂留策略见对应配置，仍须按项目约定审阅。文件存在或本地部分通过均不等于这些例外已批准；不通过增加全局豁免来掩盖违规。

两套 CI 均以只读 GITHUB_TOKEN 执行；checkout 完成后不为后续步骤保留其 Git 凭据。Rust/Gradle 缓存仅允许 push/main 回写，其他触发只读。缓存及报告仅用于工程验证，不应存放凭据；这些配置不替代仓库的外部 PR 审批、分支保护及人工安全核对。
