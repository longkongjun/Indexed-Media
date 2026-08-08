# M2 本地开发

M2 锁定 Node 24.18.0、pnpm 11.10.0、Rust 1.97.0 与仓库 lockfile。开发期允许 Vite 与 Core 两个进程以便热更新；生产镜像始终由一个 Core 进程同源提供 Vue 静态资源、REST 与 SSE。

## 本地进程

安装依赖时使用仓库声明的精确 pnpm 版本并保持 frozen lockfile：

```sh
pnpm install --frozen-lockfile
pnpm --filter @mediaflow/web typecheck
pnpm --filter @mediaflow/web exec vite
```

Core 使用 `rust-toolchain.toml`，开发配置至少提供可写临时 config、真实 deployment-roots 文件、开发 HTTP origin 和 Web dist 路径。不要让 Vite 代理到局域网或公网 Core；开发代理只应指向本机回环地址。生产同源行为由 Docker Compose 门禁验证，不能用双进程开发结果代替。

## 定向检查

```sh
just check-m2
just check-web
just check-api-client-ts
just check-test-fixtures
just check-contracts
just test-core-events
just check-containers-static
just check-compose-static
just check-containers
just check-compose
just check-nas
```

`just check-m2` 按固定顺序汇总已初始化层，并在静态检查后进入 `check-containers`、`check-compose` 和 `smoke-m2-compose` 三个 Docker-backed 门禁；任何失败都会保留非零退出码，不调用未初始化的移动端、TV、KMP、Flutter 或 labs 检查。`check-containers` 和 `check-compose` 需要可用的 Docker daemon，并构建/解析 `linux/amd64` 产物；Docker 缺失时失败是运行门禁，不是可跳过的成功。只检查文本边界可分别运行 `just check-containers-static` 和 `just check-compose-static`，但这两条不能证明镜像或 Compose 可运行。

当前单镜像/单服务 Compose 定义和文档已在仓库内；本机没有 Docker runtime 时不得把静态门禁结果写成镜像、冷启动或重启通过。绿联 UGOS Pro 的挂载、UID/GID、HTTPS、扫描、恢复清单仍须在目标 NAS 人工执行。

本地生产拓扑和 secret 准备见[M2 部署](../operations/m2-deployment.md)。

## M3 Change 1 本地识别

M3 识别继续使用同一 Core 进程和 SQLite。启动顺序是：校验 deployment roots、迁移/打开数据库、加载或无覆盖创建 `/config/instance.key`、物化 pending processing request、恢复过期 scan/processing lease、准备 discovery reconcile，最后才启动后台运行时与 HTTP 服务。实例密钥必须是常规单链接文件、恰好 32 字节且权限为 `0600`；不要通过环境变量传 TMDB Token，也不要把 `instance.key`、Token 或密文写入日志/fixture。

发现默认要求文件年龄至少 60 秒，并在间隔至少 30 秒的两次相同观察后标为稳定；周期对账默认 15 分钟。watcher 使用 1024 有界 channel 和最多 4096 个合并路径，失败、溢出或禁用时降级到完整对账，不能把 watcher 可用性当作正确性的唯一来源。ProcessingTask 默认并发 20，TMDB HTTP 并发 4；连接/总超时 5/15 秒，响应最大 2 MiB，search 最多 3 页/20 候选。NFO 最大 1 MiB、深度 32、单字段 64 KiB，只通过能力文件系统读取。

自动测试不访问公网：TMDB 测试只绑定本机假服务，NFO/媒体测试只使用临时能力根。真实 TMDB Token、真实 NAS 事件质量与权限变化必须另行人工验证。Change 1 没有媒体/NFO/Catalog 写路径，也没有 Web 人工决定 UI；`confirmed` 仅停在 `planning/identification-complete`。

定向与最终检查：

```sh
just test-m3-identification-capacity
just check-m3-identification
MEDIAFLOW_M3_BENCH_OUTPUT=/absolute/new-result.json just bench-m3-identification
```

容量命令要求输出路径的父目录已存在且目标文件尚不存在。参考夹具默认生成 100,000 个 revision、50,000 个候选、2,500 个 ReviewCase 和 1,000 个跨五阶段过期任务；`seed` 与各 count 可由 `m3-identification-fixture` 参数覆盖。2026-07-23 本机 arm64 release 记录为：生成 4.601 秒、SQLite 50.445 MiB、RSS 13.953 MiB、ProcessingTask/ReviewCase 首屏 p95 6.606/5.704 ms、缓存 hit/miss 本地查询 p95 0.041/0.040 ms、1000 任务恢复 58.765 ms。该结果排除外部 TMDB 延迟与目标 NAS，不得冒充实机证据。
