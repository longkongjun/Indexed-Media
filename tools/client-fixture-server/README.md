# Client Fixture Server

把 `contracts/examples/v1/` 的 golden fixtures 按 REST/SSE 形状吐出的本地服务器，
供 Android / iOS 模拟器走通 M5 第一批六屏。

## 用法

```bash
just client-check-fixture-server    # ktlint + build（含实际HTTP端点契约回归）
just client-fixture-server          # 0.0.0.0:8642（FIXTURE_PORT 覆盖）
```

## 端点（均来自契约 fixtures）

- `POST /api/v1/sessions`：用户名任意、密码 `fixture` → 201 + 会话 cookie；其余 401
- `GET /api/v1/media-items`（首页）/ `GET /api/v1/media-items/{id}`
- `GET /api/v1/review-cases`、`/{id}`、`/{id}/candidates?q=`、`POST /{id}/decisions`（202）
- `GET /api/v1/processing-tasks`、`POST /{id}/cancel`（202 + 任务投影）
- `GET /api/v1/integrations/jellyfin`（healthy + base_url → 外部播放可用）
- `GET /api/v1/events`：SSE 循环回放事件 fixtures（5s 间隔）

## 模拟器走查（Android）

1. 启动本服务器；启动模拟器（AVD 任意 API 36）。
2. `adb install apps/android/build/outputs/apk/debug/android-debug.apk` 后打开应用。
3. 登录页默认地址 `http://10.0.2.2:8642`（模拟器访问宿主机），用户名任意、密码 `fixture`。
4. 依次：媒体墙（翻页）→ 详情（外部播放按钮）→ 确认页（搜索+提交）→ 任务页（取消）。

iOS 模拟器同理（地址用 `http://127.0.0.1:8642`，App Transport Security 已按本地开发放行明文）。
