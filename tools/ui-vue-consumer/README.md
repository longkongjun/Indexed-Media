# MediaFlow 组件工作台 · Vue

独立、可构建的基础组件验收应用。从 `@mediaflow/ui-vue` 与公开 CSS 入口依赖正式库，不引用库内部源码、Core API、SDK、路由或业务实体。所有数据均为合成示例，无业务网络请求，不保存输入或秘密。人工状态为 **待人工验收**。

## 启动和构建

在代码仓库根执行：

```bash
pnpm install --frozen-lockfile
pnpm --filter @mediaflow/ui-vue build
pnpm --filter @mediaflow/ui-vue-consumer dev
```

打开本机 `http://127.0.0.1:4201/`。生产构建与静态预览：

```bash
pnpm --filter @mediaflow/ui-vue-consumer build
pnpm --filter @mediaflow/ui-vue-consumer preview
pnpm --filter @mediaflow/ui-vue-consumer test
```

生产输出在 `tools/ui-vue-consumer/dist/`；构建缓存不进入版本控制。应用是独立源码与构建目标，可持续扩展；无需发布组件包或部署远端。

## 目录与采用方式

| 组件 | 采用方式 | 状态所有者 |
|---|---|---|
| C01 按钮 | 正式 `MfButton` | 宿主提供busy/disabled并处理activate |
| C02 字段 | 正式 `MfField` | 受控modelValue，宿主校验和secret生命周期 |
| C03 选择、C04 复选/单选 | 原生select、checkbox、radio | 宿主持有稳定value/checked |
| C05 筛选 | MfButton与aria-pressed组合 | 宿主持有筛选，不更改对象身份 |
| C06 标签、C07 行、C08 集合 | 语义文字/列表/table及独立操作 | 数据、排序和游标归宿主 |
| C09 导航 | 原生nav/a和aria-current | 路由/当前项归宿主，不冒充tablist |
| C10 模态 | 正式 `MfDialog` | open/dirty/busy/error，确认与关闭仅发事件 |
| C11 反馈 | status/alert语义与正式恢复按钮 | 消息与恢复动作归宿主 |
| C12 请求状态 | 正式 `MfRequestState` | 请求与stale归宿主，不自动重试 |
| C13 图片、C14 卡片 | 正式 `MfImage`、`MfContentCard` | URL/标题/导航由宿主提供 |
| C15 展开 | 原生details/summary | 浏览器展开及键盘行为 |
| C16 分页 | 正式按钮与宿主追加状态 | 稳定行ID、cursor/exhausted由宿主维护 |
| C17 操作菜单 | 正式 `MfMenu` | 受控open与稳定item ID，方向/Home/End、Esc/Tab/外部关闭，禁用不激活 |

不为增加组件数量机械包装成熟原语。API说明和操作清单随目录可见；基础库不会读取本应用目录或业务数据。

## 人工操作清单

1. 遍历17组目录，切换浅色、深色、系统主题；使用大字开关与浏览器200%缩放，检查窄窗和长文本。
2. 在C01反复点击/Enter/Space，开启busy和disabled，检查计数及名称，不重复激活。
3. 在C02修改地址并触发错误，检查输入保留、label/error关联和键盘事件；C03/C04检查选择和整行单次切换。
4. 在C10编辑草稿后取消：选择继续编辑或显式丢弃；提交后模拟失败，确认值保留；unknown先查询演示回执，不能直接重复确认；模态默认取消、Tab循环、Esc受状态限制，关闭回触发按钮。
5. C12切换五态与stale：旧内容可读但不可操作；C13验证缺图/坏地址/合成SVG及显式重试；C14主链接和次操作分开。
6. C15展开长路径；C16追加后旧行保留、耗尽不再追加；C17选择操作后关闭并返回触发焦点。

填写验收人、日期、源码版本、浏览器与输入方式、实际观察和通过/不通过。自动测试、构建和浏览器脚本不能代签人工通过。图片演示只加载明确合成SVG或故意不存在的本地资源，不代表生产封面接口接通。
