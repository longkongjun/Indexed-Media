# Web 与真实本机 Core 联调

这条 opt-in Playwright 旅程不注册 `page.route`，不替换 EventSource，也不预置数据库投影。桌面 Chromium 与 Pixel 7 视口分别启动独立的 Core 进程、SQLite 数据库、一次性文件和 Vite 同源代理。

先使用当前源码构建 Core，再运行：

```sh
cargo build --manifest-path apps/core/Cargo.toml --offline --bin mediaflow-core -j 2
cd apps/web
MEDIAFLOW_REAL_CORE=1 MEDIAFLOW_TEST_OUTPUT_DIR=/tmp/mediaflow-real-core ./node_modules/.bin/playwright test --config=e2e/real-core.config.ts
```

设置 `MEDIAFLOW_REAL_WEB_DIST=/absolute/path/dist` 时，使用同一测试旅程直接访问 Core 提供的生产构建静态产物，不启动 Vite；该目录只读。此时本机 Core 仍为 HTTP development 配置，不代表 HTTPS/反代 production 部署验收。

可用 `MEDIAFLOW_CORE_BINARY=/absolute/path/mediaflow-core` 指向固定二进制；测试会复制它并记录 SHA-256，避免并行构建替换正在运行的程序。需要 Node.js 自带的 `node:sqlite`（项目当前 Node 24 工具链支持）、已安装的 Playwright Chromium 和允许监听本机端口。测试不安装依赖、不调用外部服务，关闭 screenshot、video 与 trace。

覆盖：Web 创建管理员、登录/退出/重登录、空 Catalog 的真实状态、预检并创建 `incoming/inbox` 收件目录、两次真实扫描及原始文件浏览、真实 ProcessingTask 因未配置 TMDB 暂停、Web 手动重试、停止并重新启动同一 Core 数据库后的会话/任务/扫描恢复。最后以只读 SQLite 核对任务状态、两次尝试、revision 关联、手动扫描记录与 `quick_check`，核对原文件内容及 SHA-256。后台对账扫描和手动扫描分别记录。

只有临时 `.mkv` 文件内容由测试准备，内容是测试字节，不是可播放影片。发现记录、revision、扫描任务、处理任务、尝试与会话均通过生产运行时创建；数据库预置行数为零。测试通过公开策略 API 将稳定观察间隔缩短为一秒，然后刷新页面重新获取应用 CSRF；页面操作仍使用真实产品代码。

这条测试不声称完成元数据识别、审批、整理文件或非空 Catalog 闭环：无 TMDB 配置时真实产品应暂停。审批组件由现有 `approval-flow.spec.ts`/`m3-safe-organization.spec.ts` 的契约场景覆盖，Core 的本机 HTTP/FS 测试覆盖其生产执行行为，二者不能合并成未经运行的 Web 完整审批结论。也不替代 NAS、Windows、真实手机/TV 或播放验收。

每个实例在结束或断言失败后关闭浏览器页、Vite 和 Core，并删除临时数据库与文件。JSON 报告只附接口方法/路径/状态码、二进制摘要、只读数据库计数及文件摘要，不保存密码、会话 cookie、CSRF 或数据库。
