set shell := ["bash", "-cu"]

# 项目结构门禁暂时关闭，等待进入实际开发后重新评估。
check-structure:
    @printf '待定：项目结构门禁暂未启用\n' >&2
    @exit 2

# 结构门禁测试暂时关闭，不进入日常开发流程。
test-structure:
    @printf '待定：项目结构门禁测试暂未启用\n' >&2
    @exit 2

# 检查全部版本化契约、事件和示例。
check-contracts:
    @just check-openapi
    @just check-events
    @just check-contract-examples

# 验证 M3 Change 1 的 REST/SSE 契约、生成客户端和公共 fixtures。
check-m3-identification-contracts:
    @just check-contracts
    @just check-api-client-ts
    @just check-test-fixtures

# 检查 OpenAPI 契约。
check-openapi:
    @pnpm exec redocly lint contracts/openapi/mediaflow.v1.yaml

# 检查事件 JSON Schema 与版本化示例。
check-events:
    @pnpm --filter @mediaflow/test-fixtures test -- --run

# 检查跨端契约示例。
check-contract-examples:
    @pnpm --filter @mediaflow/test-fixtures test -- --run

# 检查已初始化的 Core bootstrap 基线。
check-core:
    @just test-core-bootstrap

# 验证 M2 Task 2 的 Core 启动、SQLite 生命周期和路由安全骨架。
test-core-bootstrap:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --lib
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap --test database_lifecycle

# 验证 M2 Task 3 的单管理员、会话生命周期与请求安全边界。
test-core-identity:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --lib
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap --test database_lifecycle
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap_api --test session_api --test request_security
    @just check-openapi
    @pnpm --filter @mediaflow/api-client-ts check-generated
    @just check-test-fixtures

# 验证 M2 Task 4 的部署能力根、能力文件系统和收件目录边界。
test-core-discovery:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --lib
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap --test database_lifecycle
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap_api --test session_api --test request_security
    @cargo test --manifest-path apps/core/Cargo.toml --test deployment_roots --test inbox_api --test path_security
    @just check-openapi
    @pnpm --filter @mediaflow/api-client-ts check-generated
    @just check-test-fixtures

# 验证 M2 Task 5 的持久扫描、租约恢复、API 与稳定分页。
test-core-scanning:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --lib
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap --test database_lifecycle
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap_api --test session_api --test request_security
    @cargo test --manifest-path apps/core/Cargo.toml --test deployment_roots --test inbox_api --test path_security
    @cargo test --manifest-path apps/core/Cargo.toml --test scan_api --test scan_worker --test scan_recovery --test scan_pagination
    @just check-openapi
    @pnpm --filter @mediaflow/api-client-ts check-generated
    @just check-test-fixtures

# 验证 M2 Task 6 的事务 outbox、认证 SSE、重放、保留与恢复边界。
test-core-events:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --lib
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap --test database_lifecycle
    @cargo test --manifest-path apps/core/Cargo.toml --test bootstrap_api --test session_api --test request_security
    @cargo test --manifest-path apps/core/Cargo.toml --test deployment_roots --test inbox_api --test path_security
    @cargo test --manifest-path apps/core/Cargo.toml --test scan_api --test scan_worker --test scan_recovery --test scan_pagination
    @cargo test --manifest-path apps/core/Cargo.toml --test outbox --test sse_api --test sse_recovery
    @just check-openapi
    @just check-api-client-ts
    @just check-test-fixtures

# 验证 M3 实例密钥、TMDB 加密配置、健康投影和 API 安全边界。
test-core-tmdb-config:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test instance_secret --test tmdb_config_api

# 验证 M3 file revision、稳定门禁、发现策略与辅助视频分类。
test-core-discovery-revisions:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test file_revisions --test stability_policy --test discovery_policy_api --test auxiliary_files

# 验证 M3 watcher 有界合并、启动/周期对账与降级恢复。
test-core-discovery-continuous:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test watch_coalescing --test reconcile_coordinator --test watch_recovery

# 验证 M3 单文件处理任务、活动 attempt、租约/恢复、worker 和稳定分页。
test-core-processing:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test processing_tasks --test processing_worker --test processing_recovery --test processing_pagination

# 验证 M3 Unicode 文件名、只读 Kodi NFO 和候选身份图边界。
test-core-identification-local:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test filename_parser --test nfo_parser --test nfo_security --test media_identity_graph

# 验证 M3 有界 TMDB HTTP、缓存、single-flight、语言回退和错误分类。
test-core-tmdb:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test tmdb_client --test tmdb_cache --test tmdb_language --test tmdb_security --test tmdb_config_api

# 验证 M3 不可变识别证据、强规则决定、ReviewCase 和 revision 提交复核。
test-core-identification:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test identification_decision --test identification_service --test identification_store --test review_cases --test revision_revalidation

# 验证 M3 稳定 revision 到 Worker、REST、SSE、重启与依赖恢复的纵向识别流程。
test-m3-identification-flow:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test processing_worker --test identification_service --test identification_api --test identification_recovery --test processing_sse
    @just test-core-events
    @just check-api-client-ts

# 验证 M3 确定性容量夹具、1000 任务恢复、索引计划与敏感信息反泄漏。
test-m3-identification-capacity:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test identification_capacity --test identification_security_regression

# 在调用方指定的新 JSON 路径记录真实 100k revision/50k candidate/1000 task 主机基准。
bench-m3-identification:
    @./apps/core/scripts/record-m3-identification-benchmark.sh

# 验证 M3 正式 Catalog API、分页、索引和确定性容量夹具。
test-m3-catalog:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test catalog_store --test catalog_api --test catalog_capacity

# 在一次性临时数据库上记录 50,000 条正式媒体的实际查询证据。
bench-m3-catalog:
    @./apps/core/scripts/record-m3-catalog-benchmark.sh

# 验证 M3 不可变人工决定、重识别、任务恢复、ReviewCase API 与安全边界。
test-core-manual-review:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml \
      --test manual_decisions --test manual_decision_api --test manual_decision_recovery \
      --test manual_reidentification --test m3_review_security --test identification_api \
      --test processing_tasks --test processing_recovery --test processing_task_center

# 验证 M3 正式 Catalog 规范化读模型、认证 API、容量夹具与数据库迁移。
test-core-catalog:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml \
      --test catalog_store --test catalog_api --test catalog_capacity --test database_lifecycle

# 验证 M3 人工审核、正式媒体 Web、契约与桌面/移动主流程。
test-m3-review-admin-flow:
    #!/usr/bin/env bash
    set -euo pipefail
    output_root="$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-m3-review-admin-flow.XXXXXX")"
    trap 'rm -rf "$output_root"' EXIT
    cargo test --manifest-path apps/core/Cargo.toml \
      --test manual_decision_api --test manual_decision_recovery --test manual_reidentification \
      --test m3_review_security --test identification_api --test processing_task_center \
      --test catalog_store --test catalog_api
    CI=true just check-contracts
    CI=true just check-api-client-ts
    CI=true just check-test-fixtures
    CI=true pnpm --filter @mediaflow/web typecheck
    CI=true pnpm --filter @mediaflow/web exec vitest --exclude "e2e/**" --run \
      tests/processing-tasks.test.ts tests/review-decisions.test.ts tests/media.test.ts tests/sse-state.test.ts tests/router.test.ts tests/accessibility.test.ts
    export MEDIAFLOW_TEST_OUTPUT_DIR="$output_root/playwright"
    CI=true pnpm --filter @mediaflow/web exec playwright test --config playwright.config.ts \
      e2e/m3-review-admin.spec.ts --project=chromium-desktop --project=chromium-mobile

# 以确定性夹具和一次性 50,000 媒体 release 基准验证 M3 Catalog 容量边界。
test-m3-review-admin-capacity:
    #!/usr/bin/env bash
    set -euo pipefail
    output_root="$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-m3-review-admin-capacity.XXXXXX")"
    trap 'rm -rf "$output_root"' EXIT
    cargo test --manifest-path apps/core/Cargo.toml --test catalog_capacity
    export MEDIAFLOW_BENCHMARK_OUTPUT="$output_root/m3-catalog.json"
    ./apps/core/scripts/record-m3-catalog-benchmark.sh
    sed -n '1,$p' "$MEDIAFLOW_BENCHMARK_OUTPUT"

# 汇总 M3 Change 2 自动门禁；不冒充目标 NAS、实体人工或辅助技术验收。
check-m3-review-admin:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --all-targets
    @CI=true just check-contracts
    @CI=true just check-api-client-ts
    @CI=true just check-test-fixtures
    @just check-web
    @git diff --check
    @test -z "$(git status --porcelain --untracked-files=all -- archive)"

# 在调用方显式提供的隔离 source/target 根运行 M3 文件整理 live 验收；缺项只报告 SKIPPED。
test-m3-organization-live:
    @bash apps/core/scripts/test-m3-organization-live.sh

# 汇总 M3 安全整理自动门禁；真实 mount/NAS 文件语义必须通过独立 live 入口，不由临时目录或 fake 代替。
check-m3-safe-organization:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --all-targets
    @CI=true just check-contracts
    @CI=true just check-api-client-ts
    @CI=true just check-test-fixtures
    @CI=true just check-web
    @git diff --check
    @test -z "$(git status --porcelain --untracked-files=all -- archive)"

# 汇总 M3 Change 1 自动门禁；M2 Docker/NAS 人工门禁保持暂停且不在此伪装为通过。
check-m3-identification:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --all-targets
    @CI=true just check-m3-identification-contracts
    @just check-web
    @git diff --check
    @test -z "$(git status --porcelain --untracked-files=all -- archive)"

# 对调用方显式提供的真实 qBittorrent 与 Transmission 创建并只观察隔离测试任务；不会删除任务或数据。
test-m4-downloaders-live:
    @bash apps/core/scripts/test-m4-downloaders-live.sh

# 汇总 M4 Change 1 自动门禁；真实下载器验收必须通过独立 live 命令，不由 fixture 代替。
check-m4-downloader-management:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --all-targets
    @CI=true just check-contracts
    @CI=true just check-api-client-ts
    @CI=true just check-test-fixtures
    @CI=true just check-web
    @git diff --check
    @test -z "$(git status --porcelain --untracked-files=all -- archive)"

# 在显式真实 RSS、下载器、Ollama 与隔离能力根上运行 M4 Change 2 验收；缺项报告 SKIPPED/DEFERRED。
test-m4-source-automation-live:
    @bash apps/core/scripts/test-m4-source-automation-live.sh

# 汇总 M4 Change 2 自动门禁；真实网络、NAS、下载器和模型验收必须由独立 live 入口提供。
check-m4-source-automation:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --all-targets
    @CI=true just check-contracts
    @CI=true just check-api-client-ts
    @CI=true just check-test-fixtures
    @CI=true just check-web
    @git diff --check
    @test -z "$(git status --porcelain --untracked-files=all -- archive)"

# 验证 M2 Task 10 的独立 Core kill/restart、事务和文件系统安全回归。
test-m2-recovery:
    @cargo fmt --manifest-path apps/core/Cargo.toml -- --check
    @cargo clippy --manifest-path apps/core/Cargo.toml --all-targets -- -D warnings
    @cargo test --manifest-path apps/core/Cargo.toml --test security_regression --test recovery_capacity --test scan_capacity

# 在调用方指定的唯一 JSON 路径记录真实 100k/1000 host benchmark 并执行硬预算门禁。
bench-m2:
    @./apps/core/scripts/record-benchmark.sh

# 在唯一临时目录运行 M2 Web 主流程与恢复场景；退出时清理所有 Playwright artifact。
test-m2-e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    output_root="$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-m2-e2e.XXXXXX")"
    cleanup() {
      status="$?"
      trap - EXIT HUP INT TERM
      rm -rf -- "$output_root" || true
      exit "$status"
    }
    interrupted() {
      status="$1"
      trap - EXIT HUP INT TERM
      rm -rf -- "$output_root" || true
      exit "$status"
    }
    trap cleanup EXIT
    trap 'interrupted 129' HUP
    trap 'interrupted 130' INT
    trap 'interrupted 143' TERM
    export MEDIAFLOW_TEST_OUTPUT_DIR="$output_root/playwright"
    CI=true pnpm --filter @mediaflow/test-fixtures typecheck
    CI=true pnpm --filter @mediaflow/test-fixtures test -- --run
    CI=true pnpm --filter @mediaflow/web typecheck
    CI=true pnpm --filter @mediaflow/web exec playwright test --config playwright.config.ts \
      e2e/m2-flow.spec.ts e2e/recovery.spec.ts e2e/offline.spec.ts \
      --project=chromium-desktop --project=chromium-mobile

# 检查 M2/M3 Web 契约漂移、类型、组件行为、生产体积与桌面/移动关键流。
check-web:
    #!/usr/bin/env bash
    set -euo pipefail
    output_root="$(mktemp -d "${TMPDIR:-/tmp}/mediaflow-web-check.XXXXXX")"
    trap 'rm -rf "$output_root"' EXIT
    export MEDIAFLOW_TEST_OUTPUT_DIR="$output_root/playwright"
    CI=true pnpm --filter @mediaflow/api-client-ts check-generated
    CI=true pnpm --filter @mediaflow/web typecheck
    CI=true pnpm --filter @mediaflow/web exec vitest --exclude "e2e/**" --run
    CI=true pnpm --filter @mediaflow/web exec vite build --outDir "$output_root/dist" --emptyOutDir
    node apps/web/scripts/check-budget.mjs "$output_root/dist"
    CI=true pnpm --filter @mediaflow/web exec playwright test --config playwright.config.ts

# 验证 M2 Task 7 的 Vue 应用壳、初始化和会话身份边界。
test-web-identity:
    @CI=true pnpm --filter @mediaflow/api-client-ts check-generated
    @CI=true pnpm --filter @mediaflow/web typecheck
    @CI=true pnpm --filter @mediaflow/web test -- --run tests/router.test.ts tests/identity.test.ts tests/accessibility.test.ts

# 检查全部移动端和 TV 应用；当前模块未初始化，因此返回状态 2。
check-mobile:
    @printf '未初始化：apps/android, apps/ios, apps/android-tv, apps/tvos\n' >&2
    @exit 2

# 检查 Android 应用；当前模块未初始化，因此返回状态 2。
check-android:
    @printf '未初始化：apps/android\n' >&2
    @exit 2

# 检查 iOS 应用；当前模块未初始化，因此返回状态 2。
check-ios:
    @printf '未初始化：apps/ios\n' >&2
    @exit 2

# 检查 Android TV 应用；当前模块未初始化，因此返回状态 2。
check-android-tv:
    @printf '未初始化：apps/android-tv\n' >&2
    @exit 2

# 检查 tvOS 应用；当前模块未初始化，因此返回状态 2。
check-tvos:
    @printf '未初始化：apps/tvos\n' >&2
    @exit 2

# 检查 KMP SDK；当前模块未初始化，因此返回状态 2。
check-kmp-sdk:
    @printf '未初始化：packages/kmp-sdk\n' >&2
    @exit 2

# 检查 TypeScript API Client。
check-api-client-ts:
    @pnpm --filter @mediaflow/api-client-ts typecheck
    @pnpm --filter @mediaflow/api-client-ts test -- --run
    @pnpm --filter @mediaflow/api-client-ts check-generated

# 检查 Dart API Client；当前模块未初始化，因此返回状态 2。
check-api-client-dart:
    @printf '未初始化：packages/api-client-dart\n' >&2
    @exit 2

# 检查 UI Contract；当前模块未初始化，因此返回状态 2。
check-ui-contract:
    @printf '未初始化：packages/ui-contract\n' >&2
    @exit 2

# 检查 Test Fixtures。
check-test-fixtures:
    @pnpm --filter @mediaflow/test-fixtures typecheck
    @pnpm --filter @mediaflow/test-fixtures test -- --run

# 检查 Compose Multiplatform 实验；当前模块未初始化，因此返回状态 2。
check-compose-multiplatform:
    @printf '未初始化：labs/compose-multiplatform\n' >&2
    @exit 2

# 检查 Flutter 实验；当前模块未初始化，因此返回状态 2。
check-flutter:
    @printf '未初始化：labs/flutter\n' >&2
    @exit 2

# 检查已初始化的 M2 容器、Compose 与 NAS 文档边界。
check-infra:
    @just check-containers-static
    @just check-compose-static
    @just check-nas
    @just check-containers
    @just check-compose
    @just smoke-m2-compose

# 检查容器静态定义，不代表镜像运行门禁通过。
check-containers-static:
    @./infra/containers/tests/check-image.sh --static-only

# 检查容器定义并构建、inspect 锁定的 linux/amd64 镜像。
check-containers:
    @./infra/containers/tests/check-image.sh

# 检查 Compose 静态定义，不代表 Docker Compose 运行门禁通过。
check-compose-static:
    @./infra/compose/tests/config.sh --static-only

# 使用 Docker Compose v2 解析并检查单服务部署边界。
check-compose:
    @./infra/compose/tests/config.sh

# 运行真实 Docker Compose 冷启动/重启冒烟；只读静态检查保持为独立入口。
smoke-m2-compose:
    @./infra/compose/tests/smoke.sh

# 检查 M2 已实现模块的 README 是否已收敛为真实初始化状态。
check-m2-status:
    #!/usr/bin/env bash
    set -euo pipefail
    for readme in \
      apps/core/README.md \
      apps/web/README.md \
      packages/api-client-ts/README.md \
      packages/test-fixtures/README.md \
      contracts/README.md \
      contracts/openapi/README.md \
      contracts/events/README.md \
      contracts/examples/README.md \
      infra/README.md \
      infra/containers/README.md \
      infra/compose/README.md \
      infra/nas/README.md; do
      if ! rg -q '^已初始化' "$readme"; then
        printf 'M2 模块状态尚未收敛：%s\n' "$readme" >&2
        exit 1
      fi
    done

# 汇总已初始化的 M2 层；移动端、TV、KMP、Flutter 和 labs 保持不纳入。
check-m2:
    @just check-m2-status
    @CI=true just check-contracts
    @CI=true just check-api-client-ts
    @CI=true just check-test-fixtures
    @just check-core
    @just check-web
    @just test-m2-recovery
    @just test-m2-e2e
    @just check-containers-static
    @just check-compose-static
    @just check-nas
    @just check-containers
    @just check-compose
    @just smoke-m2-compose

# 检查 NAS 安装、代理和备份恢复文档门槛。
check-nas:
    @./infra/nas/tests/check-docs.sh

# 检查可观测性定义；当前模块未初始化，因此返回状态 2。
check-observability:
    @printf '未初始化：infra/observability\n' >&2
    @exit 2

# 完整仓库检查方案待定，待实际模块初始化后再定义。
check:
    @printf '待定：完整仓库检查暂未启用\n' >&2
    @exit 2
