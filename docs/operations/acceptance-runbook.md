# MediaFlow 实机验收 Runbook

> 从 wiki `delivery/nas-deployment.md`（绿联 DXP 4800 Plus / UGOS Pro）与任务延后验收台账派生。
> 目标：在真实 NAS 上完成部署与延后验收项；每步注明验证命令与预期。
> 前置：一台可达的 NAS（或等价 Docker 主机）、本仓库（或发布镜像）、网络可达 Docker Hub。

## 0. 交付就绪自检（开发机）

```bash
just selfcheck            # 或网络受限时：just selfcheck-offline
```

预期：四段全部通过（环境/依赖/全量门禁/静态部署边界）。任何失败先修复再继续。

## 1. 镜像构建与静态检查

```bash
just check-containers     # 构建 + inspect 锁定镜像（需 Docker daemon 与外网拉基础镜像）
just check-compose        # Docker Compose v2 解析单服务部署边界
```

延后台账项：容器内 capability 网关冒烟。

```bash
just smoke-capability-compose   # mock DSH 调用链：错误令牌拒绝→合法调用→幂等重放→cursor 拉取→管理面令牌隔离
just smoke-m2-compose           # 冷启动/重启冒烟
```

## 2. NAS 部署（绿联 DXP 4800 Plus / UGOS Pro）

按 wiki `delivery/nas-deployment.md` §部署清单执行；要点：

- `/config` 必须位于本地卷（非 SMB/NFS）；
- 挂载：`/config`、`/data/incoming/...`（收件只读）、`/data/media/...`（整理目标 read-write 根）；
- 环境变量：`MEDIAFLOW_PUBLIC_ORIGIN`、`MEDIAFLOW_TRUSTED_PROXY_CIDRS`、`MEDIAFLOW_BOOTSTRAP_SECRET_FILE`；
- **A1 起**：`MEDIAFLOW_CAPABILITY_TOKEN`（≥16 字符随机值；DSH 接入契约见 `infra/dsh/README.md`）。

部署验证：

```bash
curl -fsS https://<host>/health/ready   # {"status":"ready"}
```

## 3. M3 收尾批次验收（实机抽测）

| 项 | 命令/操作 | 预期 |
|---|---|---|
| 10 个 MVP 验收场景 | 按 wiki `product/requirements.md` §3 场景逐项操作 | 全部符合 Then 描述 |
| 文件操作语义（同/跨盘移动、硬链接） | `just test-m3-organization-live`（NAS 提供隔离 source/target 根） | 全绿；无法提供时 SKIPPED 且不算通过 |
| 中文/特殊字符/长路径/重复文件/断网/重启/空间不足/备份恢复 | wiki 证据矩阵 §M3 收尾清单 | 逐项记录证据 |
| Jellyfin 独立连接 | Web 设置 → 填真实 Jellyfin 地址凭据 → 连接测试 | healthy + 版本号；停 Jellyfin 不影响整理（MVP-AC-008） |

## 4. M4 联调批次

| 项 | 命令/操作 | 预期 |
|---|---|---|
| 真实下载器 | `just test-m4-downloaders-live` | 隔离测试任务创建并观察，不删除任务 |
| RSS/定时轮询 | 配置真实 HTTPS RSS 源 → 等待 `poll_interval_seconds` 到期 | 事件生成、去重、条件游标生效 |
| Ollama 增强 | `just test-m4-source-automation-live` | 默认关闭路径 + 增强路径均结构化 |
| 出站 Webhook | `POST /api/v1/notification-webhooks/connection-tests`（真实 https 接收端） | delivered=true |
| 备份/恢复演练 | NAS 上：`POST /api/v1/system/backups` → 停机 → 清空 /config → 按平台 restore 流程恢复 → 重启 | 数据完好、任务可恢复 |

## 5. A1–A4 批次（协议与闭环）

| 项 | 命令/操作 | 预期 |
|---|---|---|
| capability 协议（真实 DSH） | 部署 DSH → 按 `infra/dsh/README.md` 配 token → 走任务查询/计划预览/审批等待/apply 全链 | 结构化信封、幂等、cursor 补发 |
| A4 降级矩阵（实机） | 停 DSH / 停 Jellyfin / 断网期间执行本地整理 | 确定性任务不受影响（已自动化 + 实机抽测） |
| 真实 NAS quarantine | 构造待删除文件 → quarantine 计划 → 执行 → 回滚 | manifest 存在、回滚恢复原位 |

## 6. 完成动作

- 每批次证据回写 wiki 证据矩阵（环境 + 命令 + 结果）；
- 全部通过后：通知队列中的 roadmap 状态对照建议由用户确认并更新 wiki；
- 延后台账中已验证项标记关闭，剩余项（如家庭成员外部播放器体验）明确归属。
