# 独立基础组件应用

组件工作台用于人工检查正式基础库的公开API、主题、变体、输入、busy/disabled、失败恢复与平台行为。每个应用是独立构建目标，使用对应平台正式库公开入口，不依赖业务SDK、Core、账户或真实配置。工作台合成操作与生产客户端、真实服务和设备验收分别记录。

| 平台 | 应用源码与说明 | 正式库 |
|---|---|---|
| Web / Vue | [ui-vue-consumer](../../tools/ui-vue-consumer/README.md) | [ui-vue](../../packages/ui-vue/README.md) |
| Android / Compose | [ui-android-consumer](../../tools/ui-android-consumer/README.md) | [ui-android](../../packages/ui-android/README.md) |
| iOS / SwiftUI | [ui-swift-consumer](../../tools/ui-swift-consumer/README.md) | [ui-swift](../../packages/ui-swift/README.md) |
| Android TV / Compose for TV | [ui-tv-consumer](../../tools/ui-tv-consumer/README.md) | [ui-android-tv](../../packages/ui-android-tv/README.md) |

完整页面设计与基础设计规范由独立的Indexed-wiki设计应用维护；本仓库只保存生产基础库、同源组件应用与必要生产消费。tvOS当前只有设计，不存在已验收的组件应用或客户端。

## 执行方式

Web从仓库根先构建公开库，再运行独立应用：

```bash
pnpm --filter @mediaflow/ui-vue build
pnpm --filter @mediaflow/ui-vue-consumer dev
```

本机默认地址为 `http://127.0.0.1:4201/`。静态交付使用consumer的build/preview命令。Android与TV按各README构建APK，选择明确设备后人工安装。iOS用XcodeGen从声明生成工程，使用实际支持的SDK与选定模拟器/设备构建。

## 检查和人工验收

工程检查保留类型、构建、公开产物消费、状态语义、事件、焦点与恢复等行为验证。禁止通过截图采集、像素比较或AI逐图方式代签验收；Playwright明确关闭screenshot/video和trace截图，DOM与文本诊断可保留。原生测试也不调用截图API。

人工分别检查主题、系统字体、窄窗/旋转、键盘/遥控/触屏、名称与读屏、取消/失败/未知结果及返回位置。记录验收人、日期、源码版本、工具/设备、步骤、实际观察和结论。工作台存在或构建成功均不能等同人工通过；缺少真实设备/服务的目标保持待验。

## 安全边界

基础库不持有认证、路由、业务实体或网络；工作台不保存真实secret。TV工作台只有本地内容，既有管理员设备token并无只读scope，不能用它冒充只读产品接入。服务端授权、系统键盘、外部播放器和真实遥控条件未满足时，产品接入出口仍开放。
