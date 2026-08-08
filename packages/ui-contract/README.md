# UI Contract

## 目标

定义跨端 UI 预期，不实现正式 UI。

## 职责

公共 UI 契约：设计 token、页面状态、交互语义、无障碍和覆盖矩阵。

## 允许依赖

`contracts`；绝不能依赖 `apps` 或 `labs`。

## 非目标

应用专属 UI、生命周期代码、平台组件实现以及对任何应用或实验项目的依赖。

## 状态

未初始化

## 后续验证

`just check-ui-contract`
