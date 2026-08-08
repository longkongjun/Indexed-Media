# NAS 基础设施

## 目标

定义面向厂商的自托管安装边界。

## 职责

厂商打包和安装说明。

M2 说明见[绿联 DXP 4800 Plus 部署与人工验收](ugreen-dxp4800-plus.md)。`just check-nas` 只验证文档门槛，不能替代 UGOS Pro 目标机器上的人工清单。

## 非目标

业务逻辑、容器镜像定义和正式应用代码。

## 状态

已初始化：UGREEN 安装、代理、备份恢复与人工验收说明已完成并通过文档门禁；目标 NAS 人工验收待执行，`just check-nas` 不是 UGOS Pro 实机结果。

## 后续验证

`just check-nas` 验证 NAS 文档门槛；仍不代表实机验收。
