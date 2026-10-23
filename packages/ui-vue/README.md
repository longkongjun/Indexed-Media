# @mediaflow/ui-vue

MediaFlow Web 正式基础库。公开提供受控按钮、字段、模态、菜单、请求展示、图片和内容卡；其余组件按实际需要采用成熟原生语义。库不依赖Core API、KMP SDK、Pinia、路由、业务实体或真实配置，不主动请求业务数据，不解释权限或文件副作用。Vue为peer依赖，生产产物不内联Vue运行时。本包private，未发布。

## 公开入口

```ts
import { MfButton, MfField, MfDialog, MfMenu, MfImage, MfContentCard, MfRequestState } from "@mediaflow/ui-vue";
import type { MfButtonProps, MfFieldProps, MfDialogProps, MfMenuProps, MfImageProps, MfContentCardProps, MfRequestStateProps, MfRequestStateValue } from "@mediaflow/ui-vue";
import "@mediaflow/ui-vue/style.css";
```

| 组件 | 输入和槽 | 输出与责任 |
|---|---|---|
| MfButton / C01 | variant=default/primary/danger/icon，type=button/submit/reset，disabled、busy；default名称槽与装饰icon槽 | activate无负载；busy保名称/焦点并拦重复激活和原生提交，disabled为原生属性；icon必须有可访问名称 |
| MfField / C02 | id、label、modelValue必填；type、hint、error、invalid、describedBy、disabled、readonly | update:modelValue只携带新字符串；attrs/原生键盘事件透传input，label与提示/错误关联；校验、密码清除和异步请求归宿主 |
| MfDialog / C10 | open、title必填；description、busy、confirmDisabled、dirty、error、confirmLabel、cancelLabel；default内容槽 | confirm无负载；request-close携带clean/dirty，宿主决定关闭与舍弃。busy拦关闭/确认，confirmDisabled仅拦确认；默认取消、原生modal背景隔离、Tab循环和关闭归还触发焦点。错误保留内容，未知结果策略归宿主 |
| MfMenu / C17 | open、label、items（稳定id、label、可选disabled） | update:open与activate(id)；受控打开，首/末项焦点、上下环绕/Home/End、禁用拦动作、Esc归触发、Tab关闭继续、外部点击关闭 |
| MfImage / C13 | alt必填（装饰图空串），可选src、ratio=poster/landscape、retryLabel；fallback槽 | load/error/retry无负载；缺图/失败保比例与名称，显式重试同一URL，换源清失败；不拼业务URL |
| MfContentCard / C14 | title必填、subtitle；artwork/title/default/actions槽 | 本身不发导航事件；宿主title槽提供链接，次动作独立，无嵌套交互；完整标题保留，视觉两行 |
| MfRequestState / C12 | state及可本地化消息；default结果槽、empty恢复槽 | retry无负载；五态加idle，loading延迟300ms消息且立即aria-busy；stale旧内容保持对比并inert；不请求或自动重试 |

MfButton默认最小44×44 CSS px、16/1.5、可换行增高；字段min44；模态max36rem且不超过视区，内容可滚动。所有组件从 `--mfd-*` 语义token取值，样式命名空间限定各自组件，不选中宿主其他元素。

## 主题与复用

`src/styles/theme.css`使用冻结的evergreen浅/暗语义值，与`labs/design-system/tokens/tokens.json`同源并由包契约测试逐项核对。显式data-theme子树优先，默认跟随系统；aurora/sepia皮肤由宿主完整token表提供。颜色、尺寸与状态在库内集中，主题偏好/持久化由消费者负责。

基础库采用普通值/受控状态/事件/槽，不保存与宿主竞争的第二份事实。字段的未知属性落到input；模态不因dirty请求而擅自关窗。选中、加载、业务结果与请求unknown分开表达，不推断提交成功。

## 独立应用与实际消费

[Vue组件工作台](../../tools/ui-vue-consumer/README.md)是独立应用，从本包产物公开入口消费，展示C01–C17的实现或成熟原语采用、API和人工操作。输入、选择、模态和恢复数据均为合成，不调用业务API。

现有媒体两页使用按钮/卡片/图片/请求状态；登录页采用字段并保留既有密码清除及错误策略；收件目录预检确认采用受控模态，创建条件、请求和权限仍由原feature/Core裁定。真实封面URL、读屏、原生或服务验收不能由工作台证明。

## 构建与检查

```bash
pnpm --filter @mediaflow/ui-vue typecheck
pnpm --filter @mediaflow/ui-vue test --run
pnpm --filter @mediaflow/ui-vue build
pnpm --filter @mediaflow/ui-vue test:consumer
```

输出`dist/index.js`、声明文件和`dist/style.css`。组件测试验证受控事件、失败恢复与焦点；包契约验证token值源、变量完整性、样式作用域和依赖；公开产物消费者验证构建输出。必要的真实浏览器键盘/模态检查与人工验收分开登记，不使用截图采集或像素比较。所有未实际人工验收项目保持待人工验收。
