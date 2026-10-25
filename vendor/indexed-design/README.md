# Indexed-Design 设计库 vendored 依赖

本目录保存 `@indexed-design/ui-vue` 的正式 tarball 副本，供 `apps/web` 以 `file:` 依赖接入。Indexed-Design 尚未发布 npm，远端 `origin/main` 也不含所选版本，因此以仓库内 tarball + `pnpm-lock.yaml` 记录的 integrity 作为唯一可复现分发方式：任何克隆本仓库的机器（含未来 CI）执行 `pnpm install` 都得到字节一致的包。

## 当前基线

| 项 | 值 |
|---|---|
| 包 | `@indexed-design/ui-vue@0.15.0`（内部依赖 `ant-design-vue@4.2.6`、`@ant-design/icons-vue@7.0.1`，来自公共 registry） |
| 源 commit | Indexed-Design `5de0ae8`（本地 main；远端 origin/main 为 `afd8d4d` = 0.14.0，不含该版本） |
| 产物 | `indexed-design-ui-vue-0.15.0.tgz`，51397 字节 |
| SHA-256 | `87bd13e171a9f3d2a229dc1d008f73794f48248e1d1dd5abb6d197c241c52682` |

## 构建与校验方式

tarball 出自干净 checkout（detached worktree），不包含 Indexed-Design 工作树中的未提交修改：

```sh
git -C <Indexed-Design> worktree add --detach /tmp/id-baseline 5de0ae8
cd /tmp/id-baseline && npm ci --ignore-scripts && npm run build:vue
npm pack -w @indexed-design/ui-vue --pack-destination /tmp
shasum -a 256 /tmp/indexed-design-ui-vue-0.15.0.tgz
```

更新依赖时：替换本目录 tarball、更新本表（commit/版本/SHA-256）、修改 `apps/web/package.json` 的 file 路径与版本号并重新 `pnpm install`。不得以本机绝对路径、`/tmp` 路径或长期软链接作为 `package.json` 依赖地址。

## 分发缺口（如实登记，不冒称可远端安装）

- npm 未发布 `@indexed-design/ui-vue`；`npm install @indexed-design/ui-vue` 不可用。
- git 依赖（`git+...#5de0ae8`）要求该 commit 已推送；当前远端只有 0.14.0（`afd8d4d`），拉取会得到旧版，禁止当作等价基线。
- 后续若推送 0.15.0+ 或发布 npm，可评估切换为 git/npm 依赖并退役本目录；切换前本目录是唯一真源。
