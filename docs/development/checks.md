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

ci/client-ci 的验证 job 使用只读 GITHUB_TOKEN；ci 的受信 main candidate 与 docker-publish 的 promote job 单独授予 packages:write，Release job 单独授予 contents:write，不向验证 job 扩散。所有 checkout 完成后均不为后续步骤保留 Git 凭据。Rust/Gradle/BuildKit 缓存仅允许 push/main 回写，其他触发不回写。缓存及报告不得存放凭据；这些配置不替代仓库的外部 PR 审批、分支保护及人工安全核对。发布与首次安装另见[运维规程](../operations/image-release-first-install.md)，不能由 CI 配置存在推定真实发布通过。

## UI交付与人工验收

MediaFlow B1–B5沿既定范围交付：设计为可运行/可修改HTML/CSS及完整规格，统一预览由人工启动/打开；生产页面后续由人工安装客户端验收。设计交付不要求当前实现全部生产页面。规范真源为Indexed-wiki的 `design/ui-system/15-delivery-and-manual-acceptance.md`。

正式库继续复用 `packages/ui-vue`、`ui-android`、`ui-swift`、`ui-android-tv`；组件工具页直接依赖各自公开入口，展示API、主题、变体和关键状态。现有 `tools/ui-android-consumer`、`tools/ui-tv-consumer`、`tools/ui-swift-consumer`作为原生工具页基础；Web对应工具页尚待建设。tvOS只设计。

组件工具页须提供构建、安装/启动方式及人工操作清单。当前工具页展示完善、统一设计预览及旧截图流程清理尚未完成；按用户快速收尾要求先登记，不继续实施。各项目人工状态均为 **待人工验收**，iOS另注明缺完整Xcode/iOS SDK。

停止自动截图采集、模型逐图审查和截图差异比较，不为该流程启动模拟器或操作设备。仍含截图/像素采集的历史UI入口在改造前停用；本次不继续运行这些入口。保留必要构建、类型检查、行为测试及非截图日志，工程结果与人工验收分别登记。构建成功、测试通过、工具页存在均不代表人工通过。

待完善范围：四平台工具页API/主题/变体/状态与操作说明；旧截图/归档内截图和活动引用、专用采集/比对脚本清理；原生像素截图测试改为非截图行为检查或人工外观项。清理时保留产品图片/图标/海报、设计源和非截图记录。跨会话执行状态由Personal Forge唯一任务台账维护，不在此建立第二套进度。

## Web同源CSP回归

Core静态HTML、安全头与Web样式nonce桥必须配套发布。锁定的`ant-design-vue@4.2.6`、`@ant-design/icons-vue@7.0.1`兼容补丁由pnpm-workspace及lockfile校验；升级前须重评补丁，不直接删除。补丁只允许正式注入器消费文档nonce，不扫描并授权任意DOM样式。

构建后，在显式授权的隔离测试Core（具有一次性管理员和只读`incoming/camera`测试目录）上运行：

```bash
pnpm --filter @mediaflow/web build
# 通过安全环境注入MEDIAFLOW_CSP_TEST_ADMIN / MEDIAFLOW_CSP_TEST_PASSWORD，不写真实凭据到命令历史。
MEDIAFLOW_DEPLOYED_URL=http://127.0.0.1:4221 node apps/web/scripts/check-deployed-csp.mjs
```

检查脚本登录真实服务，只预检目录，不确认创建、不扫描或执行文件操作；覆盖1440/390宽度、正式主题控件浅暗切换、模态样式/几何/初始取消焦点、Tab/Shift+Tab圈闭、ESC/关闭返回、刷新nonce与主题记忆。授权样式零CSP违例单列；之后主动插入无nonce/错误nonce样式、style属性和内联脚本（含样式nonce）验证阻断，预期安全控制台告警不是业务失败。脚本不采截图、视频或trace。

Core回归：`cargo test --manifest-path apps/core/Cargo.toml --test web_csp --test bootstrap`；静态服务单测单独运行`cargo test --manifest-path apps/core/Cargo.toml --lib platform::static_web`。Web类型/lint/行为/预算按原门禁执行。实际生产TLS代理、其他浏览器、读屏和人工G1须单独验收，不以此Chromium隔离HTTP结果代签。

## Core扫描与重启：正确性和总耗时分开验收

默认Rust回归保留真实OS扫描的1201文件/130子目录、原始路径/链接与每批上限，以及独立Core kill/restart的15000文件、同task/batch、恰好一次recovery和无重复观测。原生用例的5秒/30秒检查无计数进展；租约心跳、重复读数或倒退不能重置期限。首次批次放行仍保留原15秒总限，以及至少500条且尚未全部扫描的断言。另有受控算法用例在5秒内验证观测channel与目录队列背压，不能代替原生或性能验收。

原5秒原生扫描、30秒重启扫描的**总耗时预算**仍独立保留。参考环境性能入口：

```sh
python3 apps/core/scripts/record-recovery-timing.py --describe-environment > /absolute/reference.json
# 复核采集的hardware/os/filesystem/cpu_count/memory_bytes/temp_path_depth，补充实际参考名name和max_load_per_cpu（大于0且不超过1）。
MEDIAFLOW_TIMING_REFERENCE=/absolute/reference.json \
MEDIAFLOW_TIMING_OUTPUT=/absolute/new-result.json \
just bench-m2-recovery-timing
```

结果应放在源码树以外；输出父目录必须已存在，结果及同名前缀日志不能覆盖。入口先核对参考硬件/OS/文件系统/CPU数/内存/临时路径深度和1分钟负载，再以test-opt0（debug=0、incremental=0）运行两个完整原生用例；不能与既有release版100k/1000容量数字混用。每个场景测量1次完整流程，只记录单次总耗时，不声称p95或长期稳定性。每项在全部正确性断言通过后输出阶段计时，记录命令、SHA、Core源码摘要、profile、参考/实测环境和原始日志。原生扫描计时包括Scanner执行，重启计时从第二个Core启动至completed，均不包括数据集准备；不是整份测试总耗时。

退出0且status=passed才是本参考环境的两项性能通过；超预算或正确性失败退出1/status=failed；环境不匹配或负载超出参考条件退出2/status=environment-unavailable，保持待验。计时样本缺失、错误数据量、重复样本、源码中途变化不能通过；自定义Rust flags不属于该参考profile。普通正确性全绿、受控fixture完成或缺环境均不能代签本性能出口，也不证明NAS/实机/CI或人工G1/L0验收。

## CMP Desktop

`just client-check-desktop` 校验固定Design JAR/POM/module的来源指纹、篡改反例以及桌面ktlint/行为测试/编译；已纳入client-check。`just package-desktop`只构建当前宿主自包含目录。桌面CI为独立Mac/Windows矩阵，系统安全存储使用唯一随机测试项与非敏感数据并清理。当前执行证据由Personal Forge change维护；Windows运行、签名、公证、人工体验及真实Core验收不由构建或fixture代签。详见[桌面模块](../../apps/desktop/README.md)。
