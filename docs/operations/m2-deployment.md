# M2 Docker Compose 部署

M2 生产拓扑是固定版本 `mediaflow:0.2.0-m2` 的一个镜像、一个 MediaFlow 服务和一个 Core 进程。外部反向代理不属于本 Compose 项目。

## 准备部署目录

从 `infra/compose/` 复制以下文件到同一部署目录：

- `compose.yaml`
- `.env.example`（复制为 `.env`）
- `deployment-roots.example.json`（复制为 `deployment-roots.json`）
- `secrets/.gitignore`

为 `.env` 设置数字 PUID/PGID、绝对宿主路径、端口、`MEDIAFLOW_PUBLIC_ORIGIN` 和 `MEDIAFLOW_TRUSTED_PROXY_CIDRS`。生产 origin 必须是浏览器实际访问的 HTTPS scheme 与 authority；可信代理 CIDR 只能覆盖实际 Nginx Proxy Manager 来源，不能使用 `0.0.0.0/0`。

`MEDIAFLOW_CONFIG_PATH` 必须指向 SQLite 兼容的本地卷；`MEDIAFLOW_INCOMING_PATH` 是部署者显式授权的能力根，在容器中固定映射为 read-only `/data/incoming`。`deployment-roots.json` 中的 `container_path` 必须与挂载目标完全相同。不要挂载 NAS 根目录、Docker socket 或网络文件系统作为 `/config`。

创建 bootstrap.secret 时不要让值进入 shell history：

```sh
umask 077
puid=1000
pgid=1000
openssl rand -base64 32 > secrets/bootstrap.secret
chmod 0600 secrets/bootstrap.secret
chown "${puid}:${pgid}" secrets/bootstrap.secret
```

把示例中的 `1000:1000` 替换为 `.env` 使用的实际 PUID:PGID。

Compose 通过 `MEDIAFLOW_BOOTSTRAP_SECRET_FILE=/run/secrets/mediaflow-bootstrap` 读取该文件，不把原始 secret 放入环境变量。Docker Compose 对 `file` 来源 secret 使用 bind mount，其 `uid`、`gid`、`mode` 渲染字段不会改变宿主元数据；本部署因此使用显式只读 bind mount。**宿主文件的 chown 与 chmod 0600 是权限事实来源**，容器配置文本不能替代这一步。参见 [Docker Compose service secrets 说明](https://docs.docker.com/reference/compose-file/services/#secrets)。

## 构建与启动

从仓库根构建固定 `linux/amd64` tag：

```sh
docker buildx build --platform linux/amd64 --file infra/containers/Dockerfile --tag mediaflow:0.2.0-m2 --load .
docker compose --env-file infra/compose/.env -f infra/compose/compose.yaml config
docker compose --env-file infra/compose/.env -f infra/compose/compose.yaml up -d
docker compose --env-file infra/compose/.env -f infra/compose/compose.yaml ps
```

发布环境应记录镜像 digest，并用该不可变 digest 重现部署；不得改用 `latest`。服务使用只读根文件系统、`/tmp` tmpfs、全部 capability drop 和 no-new-privileges。权限失败时修正宿主目录 PUID/PGID，不得把容器改成 root、privileged 或 host network。

## 启动后

先确认 `/health/live` 和 `/health/ready`，再通过 HTTPS 初始化管理员、退出并重新登录。确认成功后按 NAS 文档处理一次性 secret。升级前必须阅读[备份与干净恢复](backup-restore.md)，反向代理按[Nginx Proxy Manager](reverse-proxy.md)配置。

## 检查与证据边界

当前固定组合为 Node 24.18.0、pnpm 11.10.0、Rust 1.97.0 和 `mediaflow:0.2.0-m2`；仓库入口 `just check-m2` 依次运行 `just check-contracts`、`just check-api-client-ts`、`just check-test-fixtures`、`just check-core`、`just check-web`、静态容器/Compose/NAS 检查，再运行 `just check-containers`、`just check-compose` 和 `just smoke-m2-compose`。Docker daemon 不可用时，后三级门禁必须失败并保留退出码，不能改写为“通过”。

本文描述的是单镜像、单 MediaFlow 服务的部署流程；当前 Mac 已完成固定 `linux/amd64` 镜像、单服务 Compose、冷启动、重启和任务结果持久化验证，但目标 DXP 4800 Plus 的 UGOS Pro 人工验收尚未执行。只有在目标 NAS 上记录镜像摘要、挂载权限、HTTPS/SSE、扫描、重启和备份恢复证据后，才能更新人工验收状态。
