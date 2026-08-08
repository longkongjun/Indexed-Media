# 绿联 DXP 4800 Plus 部署与人工验收

本文面向运行 UGOS Pro 的绿联 DXP 4800 Plus。绿联官方说明该机型可从应用中心使用 Docker，并支持通过 SSH 执行进阶 Docker Compose 工作流；界面名称可能随 UGOS Pro 版本变化，因此本文以可审计的 Compose 文件和命令为准：[UGREEN DXP4800 Plus 官方页面](https://nas.ugreen.com/products/ugreen-nasync-dxp4800-plus-nas-storage)。

## 部署前准备

1. 在 UGOS Pro 应用中心安装 Docker，开启管理员 SSH 仅用于部署阶段，并确认 `docker compose version` 可用。
2. 在 NAS 的本地存储池创建 `mediaflow/config`、`mediaflow/secrets` 和真实下载完成目录。`/config` 必须位于本地 ext4/btrfs 一类可满足 SQLite 锁与落盘语义的卷，不能放在 NFS、CIFS 或 SMB 网络挂载。
3. 选定运行 MediaFlow 的 NAS 数字用户，执行 `id <用户名>` 取得 PUID、PGID。将 config、secret 目录和文件归属到相同数字 UID/GID；不要用 root 容器绕过权限。
4. 把 `infra/compose/` 复制到部署目录，复制 `.env.example` 为 `.env`，复制 `deployment-roots.example.json` 为 `deployment-roots.json`，把其中容器路径保持为 `/data/incoming`。
5. 以 `umask 077` 创建 `secrets/bootstrap.secret`，写入至少 32 个随机字节的高熵值，随后执行 `chmod 0600` 和 `chown PUID:PGID`。不要把值粘到 Compose、`.env`、命令参数、日志或截图中。

Compose 只授予可写 `/config`、只读 `/data/incoming` 和只读根配置；不得改成 NAS 整机根目录、`/volume*` 根目录、Docker socket、宿主网络或特权模式。媒体目录保持 read-only。

## 构建并核对固定镜像

默认 tag 是仓库本地构建产物，不是公共 registry 镜像。干净安装必须先把完整仓库复制到 NAS，在仓库根执行固定 `linux/amd64` 构建并载入本机 image store：

```sh
docker buildx build --platform linux/amd64 --load \
  --file infra/containers/Dockerfile \
  --tag mediaflow:0.2.0-m2 .
docker image inspect mediaflow:0.2.0-m2 \
  --format 'architecture={{.Architecture}} image_id={{.Id}} repo_digests={{json .RepoDigests}}'
```

启动前确认 `.Architecture` 为 `amd64`，记录 `.Id` 的 `sha256:` 值和 `.RepoDigests`（本地未推送镜像可能为空数组），并把记录与 UGOS Pro 版本一同保存。`.env` 的 `MEDIAFLOW_IMAGE` 必须仍指向刚核对的固定 tag；不得回退到 `latest`。

## UGOS Pro 启动

在部署目录先运行：

```sh
docker compose --env-file .env -f compose.yaml config
docker compose --env-file .env -f compose.yaml up -d
docker compose --env-file .env -f compose.yaml ps
```

如果 UGOS Pro 的“项目”界面支持导入 Compose，也应导入同一 `compose.yaml` 和 `.env`；导入前仍需在文件管理器或 SSH 中完成数字 PUID/PGID、挂载目录及 secret 的 `0600` 权限。健康检查失败时先检查 `/config` 的实际写权限、根配置文件与真实只读挂载，不要切换 root 用户重试。

外部 HTTPS 按[反向代理说明](../../docs/operations/reverse-proxy.md)配置：已有域名时可以使用 Nginx Proxy Manager；只有局域网 IP 时使用独立 Caddy 项目和内部 CA。两种方式都必须设置可信公开 origin、最窄代理来源 CIDR 和 SSE 长连接；不要直接把内部 HTTP 端口暴露到局域网或公网。

## 一次性初始化密钥

浏览器初始化成功后，管理员存在状态不可逆地消费原密钥。确认可以重新登录后删除宿主机 `bootstrap.secret`；若以后需要重新创建容器，先生成一个新的临时随机文件以满足只读 bind mount，已有管理员不会因此再次开放初始化。该临时文件仍须归属 PUID:PGID 且为 `0600`。

## DXP 4800 Plus 人工验收清单

下列步骤必须在目标 NAS 上实际执行并记录镜像摘要、UGOS Pro 版本、文件系统、PUID/PGID 和结果；本文存在不等于验收完成。

- [ ] 固定 `linux/amd64` 镜像 tag 与摘要，容器内 UID/GID 与配置的 PUID/PGID 一致且非 0。
- [ ] `/config` 是本地卷、可写并在容器重启后保留 SQLite 数据；`/data/incoming` 是真实目录的只读挂载。
- [ ] secret 文件归属运行 UID/GID、权限为 `0600`，值未进入镜像、Compose、`.env` 或日志。
- [ ] 只通过配置的 HTTPS origin 初始化和登录，安全 Cookie、REST 与 SSE 同源。
- [ ] Caddy 或 Nginx Proxy Manager 关闭响应缓冲并保持 SSE 长连接，Core 只信任实际代理 `/32` 或最窄 CIDR。
- [ ] 添加 `/data/incoming` 下的目录、启动扫描并看到累计进度、单项错误和原始文件。
- [ ] 重启容器后同一管理员、收件目录、任务和文件结果仍存在，重复文件记录没有增加。
- [ ] 按备份文档生成可验证副本，并恢复到新的空 `/config` 后核对管理员、收件目录和任务计数。

当前控制器没有 Docker，且目标 DXP 4800 Plus 尚未验证以上清单；Task 9 必须保持“尚未验证”，不能把静态配置检查描述为 NAS 验收通过。
