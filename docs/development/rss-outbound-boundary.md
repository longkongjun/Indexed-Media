# RSS 与通知的公网出站边界

RSS 订阅与出站通知只连接 HTTPS 公网目的。下载器的内网管理接口属于另一个明确配置的 connector 边界，不受这里的 feed 策略放行。

`platform/outbound_http.rs` 统一校验 URL 和 IP：拒绝用户信息、fragment、端口 0、回环、私网、链路本地、共享/保留/文档地址、映射 IPv6，以及单标签/localhost 目标。每次请求解析全部地址；空结果、超过 16 个地址或其中任何地址非公网均拒绝。实际连接通过 `resolve_to_addrs` 使用同一批经过校验的地址，不再进行另一轮目的 DNS 解析。

URL 的主机名保持原值用于 Host 和 TLS 证书验证；没有关闭证书或主机名校验。请求禁用环境代理，不自动跟随重定向，管理员应自行核实来源迁移。源 URL 和查询参数可能含私密订阅标识，不写入错误、诊断或日志；adapter 返回原有稳定错误枚举。

RSS 仍保留 5 秒连接上限、默认 15 秒请求总预算、2 MiB 响应上限和共享的最多 4 个并发许可。请求总预算同时覆盖许可等待、DNS、连接及读取响应；poller 的整体截止时间另由现有调度器维护。通知保留原 10 秒整体预算。每个生产请求建立绑定当前已验证 DNS 的客户端，不借旧连接池跳过本次地址检查。

测试中 `FeedHttpClient::for_test_loopback` 仍仅接受显式 HTTP 回环地址，不是生产放行私网开关。私有 pinned-helper 的本机测试只证明连接固定、Host 保持和不跟随重定向；不冒称真实公网 TLS、代理部署或 NAS 已验收。生产拒绝私网的回归使用测试自己创建的监听器，并断言未发生 TCP 连接，不读取实际 feed 配置或凭据。

主要自动检查：`cargo test --locked --offline --manifest-path apps/core/Cargo.toml --lib --test rss_outbound --test rss_poller --test rss_poll_fencing --test notification_delivery`。真实外部源及部署条件应独立验证，自动检查不能代替人工验收。
