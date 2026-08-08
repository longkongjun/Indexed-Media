# MediaFlow Web Prototype

M1.4 中保真交互原型，只验证管理员 Web 的流程、状态和响应式布局。

## 运行

```bash
python3 -m http.server 4173 --directory labs/web-prototype
```

打开 `http://127.0.0.1:4173/`。

## 验证

```bash
node --test labs/web-prototype/tests/*.test.mjs
node labs/web-prototype/scripts/check.mjs
```

原型只使用本地模拟数据，不连接 Core、文件系统、TMDB 或 Jellyfin。正式应用不得引用本目录。

## 原型流程

- P1 首次配置：`#/setup?step=1`；Jellyfin 是设置中的可选独立连接
- P2 本地自动成功：`#/overview`、`#/task/task-auto`
- P3 人工确认与通用视频分组：`#/task/task-review`
- P4 安全暂停：`#/task/task-conflict`、`#/task/task-failed`
- P5 本地部分成功与连接隔离：`#/task/task-partial`、`#/settings`
- P6 中断恢复：`#/task/task-recovery`
- P7 本地结果查看：`#/media`、`#/media-detail/media-movie`

## 状态预览

在任意页面 hash 后添加 `state` 参数：

- `#/overview?state=loading`
- `#/tasks?state=empty`
- `#/overview?state=error`
- `#/overview?state=offline`
- `#/tasks?state=conflict`
- `#/media?state=partial`

这些入口只用于原型验收，不改变模拟数据。
