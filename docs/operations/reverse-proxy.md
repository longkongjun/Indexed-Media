# 局域网 HTTPS 反向代理与 SSE

MediaFlow Core 只监听容器内部 HTTP，生产访问必须由容器外的反向代理终止 TLS。浏览器使用的地址必须与 `MEDIAFLOW_PUBLIC_ORIGIN` 完全一致；Core 只信任 `MEDIAFLOW_TRUSTED_PROXY_CIDRS` 中的实际代理来源。

M2 支持以下两种外部代理方式：

- 已有域名和证书管理设施时使用 Nginx Proxy Manager。
- 只有局域网 IP、没有域名时使用独立 Caddy 项目和 Caddy 内部 CA。

反向代理不属于 MediaFlow 镜像或 MediaFlow Compose 服务。增加独立 Caddy 容器不会改变“单 MediaFlow 镜像、单 MediaFlow 容器、单 Core 进程”的产品边界。

## Caddy 局域网 IP HTTPS

本节用于仅在可信局域网访问 `https://192.168.1.79:8443` 的目标机验收。使用 [Caddy Docker Official Image](https://hub.docker.com/_/caddy) 的固定稳定镜像 `caddy:2.11.4-alpine`，通过离线镜像归档部署，避免把 Docker Hub 可用性变成 NAS 启动依赖。部署后仍应记录实际镜像 ID 或 RepoDigest，不能改用 `latest`。

### 网络与端口边界

1. 为 MediaFlow 与 Caddy 创建一个未与 NAS 现有 Docker 网络重叠的专用 `/29` bridge 网络；创建前必须先检查 `docker network inspect` 的现有子网。
2. 给 Caddy 分配该网络内的固定 IPv4 地址，并把 `MEDIAFLOW_TRUSTED_PROXY_CIDRS` 精确设置为这个地址的 `/32`。不得信任整个 Docker 私网、NAS 局域网或 `0.0.0.0/0`。
3. MediaFlow 仍只暴露诊断端口 `127.0.0.1:13000:3000`，不得把内部 HTTP 暴露到局域网或公网。Caddy 只发布 `8443:8443`，且只能由可信局域网访问。
4. Caddy 通过专用 Docker 网络和服务别名 `mediaflow:3000` 转发，不通过宿主公开端口绕行。

MediaFlow 使用主 Compose 文件和一个只负责加入外部网络的部署覆盖文件。覆盖文件不得新增第二个 MediaFlow 服务。Caddy 使用独立 Compose 项目和独立目录，例如 `/volume1/docker/mediaflow-m2/caddy`；其数据卷不得挂入 MediaFlow 容器。

### Caddy 配置

局域网 IP 证书使用 Caddy [`tls internal`](https://caddyserver.com/docs/caddyfile/directives/tls) 内部 CA。`Caddyfile` 的核心配置如下：

```caddyfile
{
	admin off
	auto_https disable_redirects
	default_sni 192.168.1.79
}

https://192.168.1.79:8443 {
	tls internal
	reverse_proxy mediaflow:3000 {
		flush_interval -1
	}
}
```

IP 字面量客户端可能不发送 TLS SNI，而 Caddy 在 Docker 网络中观察到的本地地址是容器地址；`default_sni 192.168.1.79` 因此必须与公开 IP 保持一致，否则无 SNI 客户端无法选择已签发证书。Caddy 默认设置并维护 `X-Forwarded-For`、`X-Forwarded-Host` 和 `X-Forwarded-Proto`。[`flush_interval -1`](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy#streaming) 明确关闭代理响应缓冲，使 SSE 事件和 15 秒 heartbeat 立即下发；不要额外启用会聚合 `text/event-stream` 的压缩或缓存层。

Caddy Compose 以非 root UID/GID 运行并先 `cap_drop: [ALL]`。官方镜像的 Caddy 二进制带 `cap_net_bind_service` 文件 capability，Linux 要求 capability bounding set 包含对应项才能执行；Compose 因此只加回 `cap_add: [NET_BIND_SERVICE]`。这不授权 privileged、host network、Docker socket 或其他 capability，实际服务仍只发布高位端口 8443。

Caddy 的 `/data` 包含局域网根 CA 私钥，宿主目录必须只允许运行 Caddy 的非 root UID/GID 访问，且不能进入仓库、部署包、日志、截图或 Mac。只把容器内 `/data/caddy/pki/authorities/local/root.crt` 对应的公开根证书复制到需要访问的客户端。

### 当前 Mac 的临时证书信任

首次启动 Caddy 后，从 NAS 的 Caddy 数据目录复制公开 `root.crt`，核对证书指纹，再把它临时加入当前 Mac 的系统钥匙串。导入系统钥匙串会改变本机信任配置，必须由 Mac 管理员明确执行；验收结束后按同一指纹删除该根证书。不要复制或导入 Caddy 的根 CA 私钥。

证书信任只解决浏览器验证问题，不替代 origin 校验。MediaFlow 必须设置：

```dotenv
MEDIAFLOW_PUBLIC_ORIGIN=https://192.168.1.79:8443
# 仅当部署前检查确认 172.30.79.0/29 未被占用时使用此地址。
MEDIAFLOW_TRUSTED_PROXY_CIDRS=172.30.79.3/32
MEDIAFLOW_BIND_ADDRESS=127.0.0.1
MEDIAFLOW_PORT=13000
```

`172.30.79.3/32` 是条件化的部署值，不是默认信任范围。若预检发现 `172.30.79.0/29` 重叠，必须选择另一个未占用 `/29`，并把 Caddy Compose 的固定地址与此处 `/32` 同步替换后再启动。

### Caddy 验收与回滚

先通过 `https://192.168.1.79:8443/health/live` 和 `/health/ready` 验证代理与 Core，再在同一 origin 完成初始化、退出、重新登录和 SSE 重连。浏览器开发者工具中必须看到 `__Host-mediaflow_session` 具有 `Secure`、`HttpOnly`、`Path=/`，EventSource 长时间保持连接且 heartbeat 不被批量释放。

回滚时先停止 Caddy 项目，再停止 MediaFlow 项目；保留 `/config`、Caddy 数据目录和媒体只读挂载以便调查，不删除或改写媒体文件。恢复旧代理前先把 `MEDIAFLOW_PUBLIC_ORIGIN` 和可信代理 `/32` 改回对应值，然后重建 MediaFlow。目标机验收完成后撤销临时 Docker socket ACL、部署 SSH 公钥和 Mac 根证书信任。

## Nginx Proxy Manager

已有域名和证书管理设施时，可以由外部 Nginx Proxy Manager 终止 TLS。

## Proxy Host

在 Nginx Proxy Manager 创建 Proxy Host，Forward Hostname/IP 指向 NAS 或 MediaFlow 可达地址，Forward Port 使用 Compose 暴露端口，Scheme 为 `http`。申请证书、启用 Force SSL，并只允许受信网络访问管理面。

SSE 使用普通长连接 HTTP，不是 WebSocket；“WebSocket Support”选项不是必需条件。无论该选项是否开启，都必须在 Advanced 配置关闭 SSE buffering、缓存与压缩缓冲，并延长读超时：

```nginx
proxy_buffering off;
proxy_cache off;
proxy_read_timeout 3600s;
proxy_send_timeout 3600s;
gzip off;
proxy_set_header X-Forwarded-Proto $scheme;
proxy_set_header X-Forwarded-Host $host;
```

不要重写 `/api/v1/events`，也不要把 Web、REST 和 SSE 分到不同 origin。确认浏览器只访问 `https://<域名>/`，EventSource 长时间保持连接，15 秒 heartbeat 不被代理聚合；断开后浏览器应携带 `Last-Event-ID` 重连。

## 可信代理边界

从 Core 容器观察实际 TCP 对端地址或 Nginx Proxy Manager 所在 Docker 网络，配置最窄 CIDR。更换代理网络后先更新该值再重建 MediaFlow；不要信任整个地址空间，也不要直接把内部 HTTP 端口暴露到公网。

## 检查与证据边界

反向代理说明属于固定 `mediaflow:0.2.0-m2` 单镜像、单 MediaFlow 服务拓扑的外部部署边界。自动检查使用锁定的 Node 24.18.0、pnpm 11.10.0、Rust 1.97.0 工具链，并通过 `just check-nas` 检查本文的 HTTPS/SSE 文档门槛；完整汇总入口为 `just check-m2`，其中 `just check-containers`、`just check-compose` 和 `just smoke-m2-compose` 需要 Docker runtime。文档检查或静态配置检查不能证明 Caddy 或 Nginx Proxy Manager 已在目标 NAS 上实际转发 SSE。
