# 原生品牌接入

正式 MediaFlow 使用 Indexed-Design 已选 `media` 图形与完整产品名；Android TV 是平台名称，不能因此换用直播 `tv` 图形。导航/连接采用正式 `IndexedBrandLockup`，手机与桌面关于、TV 设置中的归属采用 `Endorsed` 纵向组合。应用现有主题入口和默认值保持不变。

## 组件与资源来源

Android、Android TV、Desktop 分别固定消费 `compose`、`compose-tv`、`compose-desktop:0.15.0`。制品保存在 `vendor/indexed-design/maven`，三个 `*-provenance.json` 记录完整来源提交和逐文件 SHA-256。Gradle exclusiveContent 将这些坐标限定到仓库内制品，克隆后不需要邻仓源码或 build 目录。

`vendor/indexed-design/brand/media.svg` 和 `media-foreground.xml` 分别来自已选 SVG 和已交付平台转换。`tools/brand/build-native-assets.py` 仅以相同原形生成固定中性宿主资源：背景 `#242326`、图形白色。它不是 violet/jade/wine/cocoa 中某一套的新默认主题；没有修改图形路径或专属字形。

| 平台/页面 | 正式入口 | 验证入口 |
| --- | --- | --- |
| Android 导航/登录/关于 | `apps/android/.../MainActivity.kt` | `just client-check-android`，设备另验 |
| Android TV 导航/连接 | `MainActivity.kt`、`TvInput.kt` | `just client-check-android-tv`，遥控/远距另验 |
| Android TV 关于/设备 | `TvSettings.kt` | 同上；保留主题与安全退出流程 |
| Desktop 导航/连接/关于 | `Main.kt` | `just client-check-desktop`、`DesktopBrandTest` |
| Android/TV 应用图标 | manifest → `mipmap-anydpi-v26/v33/ic_launcher.xml` | 资源编译、生成摘要；实际 launcher mask 另验 |
| Android TV banner | manifest → `drawable-nodpi/brand_banner.png` | 320×180源资源与编译；远距阅读另验 |
| Desktop 窗口/安装图标 | `brand/icon.png`、`icons/icon.icns/.ico`、Gradle配置 | Desktop编译与源资源检查；Mac/Windows安装分别验 |

Android 12+ 系统启动页沿用已配置 launcher icon 的平台默认机制，没有追加品牌背景或状态动效。旧平台启动外观与系统裁切需实际设备检查，不能由资源编译推断。Desktop 不新增自定义启动页。

## 重建与检查

已有资源检查只需 Python 标准库：

```sh
python3 tools/brand/build-native-assets.py --check
python3 tools/desktop/check-vendor.py android
python3 tools/desktop/check-vendor.py tv
python3 tools/desktop/check-vendor.py
python3 -m unittest discover -s tools/desktop -p 'test_*.py'
```

重建 PNG/ICO/ICNS 另需 `rsvg-convert` 与 Pillow，运行 `python3 tools/brand/build-native-assets.py`。它只读取仓库内固定源，生成清单记录来源与每个输出摘要；不能用重新生成清单绕过源图形审查。更新源或制品时先对照对应 Indexed-Design 完整提交再更新来源清单。

平台 mask、真实设备、TV 遥控与远距、读屏、系统极端字号、安装包和人工视觉必须独立验收；编译、JVM测试、格式与摘要通过均不等于这些验收通过。iOS/tvOS接入与验证单独管理。
