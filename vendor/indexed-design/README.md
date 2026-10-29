# Indexed-Design 设计库 vendored 依赖

本目录保存 `@indexed-design/ui-vue` 的正式 tarball 副本，供 `apps/web` 以 `file:` 依赖接入。Indexed-Design 尚未发布 npm，远端 `origin/main` 也不含所选版本，因此以仓库内 tarball + `pnpm-lock.yaml` 记录的 integrity 作为唯一可复现分发方式：任何克隆本仓库的机器（含未来 CI）执行 `pnpm install` 都得到字节一致的包。

## 当前基线

| 项 | 值 |
|---|---|
| 包 | `@indexed-design/ui-vue@0.15.0`（内部依赖 `ant-design-vue@4.2.6`、`@ant-design/icons-vue@7.0.1`，来自公共 registry） |
| 源 commit | Indexed-Design `9d0038c16688a4a74058b3b8f0d7899d4aadbbdb`（品牌正式API与主题，包版本仍为0.15.0；以完整SHA与tarball字节区分） |
| 产物 | `indexed-design-ui-vue-0.15.0.tgz`，65239 字节 |
| SHA-256 | `1770ee5e7b55dc39495c22b2d8dc879e577980298e306e42e12012b2d6873782` |

Web品牌来源与favicon转换记录见 `web-brand-provenance.json`。此更新不更改宿主默认主题。

## 构建与校验方式

当前tarball从上述完整source commit对应、受跟踪文件无修改的Indexed-Design源码重建；未跟踪的`.idea/`不进入npm包。以下是隔离复现方式：

```sh
git -C <Indexed-Design> worktree add --detach /tmp/id-baseline 9d0038c16688a4a74058b3b8f0d7899d4aadbbdb
cd /tmp/id-baseline && npm ci --ignore-scripts && npm run build:vue
npm pack -w @indexed-design/ui-vue --pack-destination /tmp
shasum -a 256 /tmp/indexed-design-ui-vue-0.15.0.tgz
```

更新依赖时：替换本目录 tarball、更新本表（commit/版本/SHA-256）、修改 `apps/web/package.json` 的 file 路径与版本号并重新 `pnpm install`。不得以本机绝对路径、`/tmp` 路径或长期软链接作为 `package.json` 依赖地址。

## 分发缺口（如实登记，不冒称可远端安装）

- npm 未发布 `@indexed-design/ui-vue`；`npm install @indexed-design/ui-vue` 不可用。
- 2026-10-26 起 Indexed-Design 远端 `origin/main` 已含该基线（本地推送 `afd8d4d..60da115`），git 依赖技术上可达；但 npm 仍未发布，且 git 依赖不提供锁文件级字节校验，本仓库继续以 vendored tgz + integrity 为唯一分发真源。后续若发布 npm 或统一改用 git 依赖，可评估退役本目录；切换前本目录是唯一真源。

## CMP Desktop 0.15.0

`maven/dev/indexed/design/compose-desktop/0.15.0` 保存尚未发布公共Maven的桌面库 JAR/POM/Gradle module 三个固定产物。`desktop-provenance.json` 记录完整来源 commit 和逐文件 SHA-256。消费端通过 Gradle exclusiveContent 只从此目录解析该模块，不回退公共同名包。

从对应来源提交的 Indexed-Design `platforms/desktop` 执行 `./gradlew :library:publishDesktopPublicationToProjectLocalRepository` 后拷贝这三个产物并更新清单。更新需要重新运行 `python3 tools/desktop/check-vendor.py`、篡改反例和桌面消费者测试；JAR 内不包含业务 SDK 或凭据。此目录不声称已发布远程Maven。
