# Foundation studio · Android TV

独立组件与基础规范应用，最低 Android 26，公开消费正式库。仅使用本地合成数据，不依赖 KMP SDK、Core、业务实体、账户或真实配置。所有未实际人工操作项均为 **待人工验收**。构建、行为测试、真实设备与人工验收分别记录；不采集截图或运行像素比较。

## 构建、安装与启动

从代码仓库根运行：

```bash
./gradlew :tools:ui-tv-consumer:assembleDebug
adb -s <device-serial> install -r tools/ui-tv-consumer/build/outputs/apk/debug/ui-tv-consumer-debug.apk
adb -s <device-serial> shell am start -n example.uifoundation.tv/example.uifoundation.tv.MainActivity
```

只安装到明确选择的目标。TV 应用使用 Leanback 启动入口，真实遥控、系统密度和远距阅读须在 Android TV 系统上验收，普通手机模拟器不替代。

## 公开入口与状态所有者

`mediaflow.ui.tv` 提供 `MfTvTheme`、`MfTvButton`、`MfTvContentCard`、`MfTvMessage`。官方 Compose for TV 提供控件语义与焦点。基础按钮/卡片不几何缩放，焦点边界与选中独立；标题和说明使用 TV 语义字阶，滚动留安全边距。状态和稳定条目 ID 由消费者持有。

## 人工操作

1. 用 D-pad 上下左右遍历工作台，Center 激活；切换浅/暗主题，检查焦点描边、正文及边界。
2. Reserve seat 只累计一次；busy 保留名称且不能重复，Simulate failure 允许恢复；Unavailable 不能激活。
3. 打开任一卡片，完整标题可读；按 Back 回到同一 ID；删除最近条目后恢复最近存活项，删空后 Reset sessions 可达，重置回首项。
4. 滚动到最后一行，确认底部内容和按钮可达；放大系统字体，不裁长标题说明。
5. 在明确选定的 TV 1080p/4K 目标记录实际逻辑窗口、density、遥控器与远距阅读观察；普通 Android 模拟器只算局部行为环境。

TV 连接/搜索的系统键盘、只读设备授权、真实服务、外部播放器及返回属于产品接入验收。本应用不接入设备 token，不以隐藏写操作冒充服务端只读授权。tvOS 本期仅设计。

## TV 13 组采用映射

| 组 | 正式实现或平台采用 | 工具应用与验收边界 |
|---|---|---|
| C01 操作 | `MfTvButton` 正式实现 | busy / disabled / 失败恢复 |
| C05 筛选 | Compose for TV Button 与宿主选中值 | 生产筛选与刷新焦点由业务壳验收 |
| C06 标签 | TV `Text` 语义组合 | 非交互状态文字不接受焦点 |
| C07 信息 | TV `Text` 与宿主布局 | 完整标题与长说明 |
| C08 集合 | Compose `Row` / 滚动 `Column` | 稳定 ID 网格、删除近邻与删空恢复 |
| C09 导航 | Compose focus / `BackHandler` 宿主组合 | 详情与返回原 ID；产品顶栏另验 |
| C10 模态 | Compose `Dialog` 与 TV 控件组合 | 成熟原语采用；产品弹窗未由工具构建代签 |
| C11 反馈 | `MfTvMessage` 正式实现 | 常驻失败与恢复说明 |
| C12 请求状态 | `MfTvMessage` 与宿主状态 | 失败保内容；服务断连另验 |
| C13 图像 | 宿主 artwork 槽 | 加载、失败与网络授权由宿主，工具不联网 |
| C14 内容 | `MfTvContentCard` 正式实现 | 长标题、装饰槽与原生焦点 |
| C15 展开 | TV Button 与宿主内容组合 | 产品长读区另验 |
| C16 更多 | TV Button、稳定 key 与宿主集合 | 焦点恢复样例；真实游标追加另验 |

C02 系统输入须由 B5 设计明确系统键盘桥接、取消与焦点归还；不复用手机 M3 输入。C03/C04/C17 不在当前 TV 13 组基线内。tvOS 只维护设计，无生产包。采用映射说明复用方法，不代表全部状态已在 TV 系统、遥控或真实服务上通过。
