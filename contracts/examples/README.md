# 契约样例

## 目标

提供共享 payload 样例，使跨端契约行为具体可见。

## 职责

所有客户端共享的有效与无效 payload 样例。

## 非目标

正式 fixture 执行、应用专属测试和业务逻辑。

## 状态

已初始化：`v1/` 包含 M2 与 M3 Change 1 的正样例及敏感字段泄漏反样例。M3 样例只含有界 provider 投影和稳定原因码，不包含 Token、密文、实例密钥、NFO 原文、provider 原始响应或绝对主机路径。

## 后续验证

`just check-contract-examples`
