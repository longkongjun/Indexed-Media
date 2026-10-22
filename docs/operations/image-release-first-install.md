# 镜像发布与空白 NAS 首次安装

本入口只交付 `linux/amd64` Core/Web 镜像和全新实例。已有目录即使为空也拒绝首装；不能用于升级、接管、维护、数据恢复或自动删除现场。完整交付约定见 [镜像分发与更新方案](https://github.com/longkongjun/Indexed-wiki/blob/main/delivery/image-pipeline.md)。

## 本地验证与实际运行的分工

`just check-release-tools check-gate-regressions check-containers-static check-compose-static check-nas` 执行发布/安装边界回归与静态检查；测试中的 registry、Docker 和 GitHub 替身只验证编排，不证明实际部署。`just check-containers` 会构建一次镜像；已构建产物使用 `MEDIAFLOW_TEST_IMAGE=mediaflow:sha-完整SHA ./infra/containers/tests/check-image.sh --existing-image`，随后以同一变量运行 `just check-compose smoke-m2-compose`。冒烟覆盖非root、默认进程、Web/API、认证SSE、冷启动及重启后的任务持久化，并只清理隔离夹具。

当前本地环境缺少Docker时，运行时检查必须失败，不能加skip/continue-on-error或把历史CI当作本版本结果。真实首次安装、GHCR网络、HTTPS/代理和NAS权限仍需在相应环境验收。

## 候选与正式发布

1. 可信仓库main push运行 `ci.yml`。Rust、依赖、契约、Web及infra全部成功后，候选job才有packages写权限；PR只验证。infra用固定基础镜像与Action构建一次、执行实际镜像/Compose冒烟，再保存同一镜像。Rust、Gradle与BuildKit缓存仅允许main push回写，checkout不保留凭据。
2. 候选发布固定 `mediaflow-dev:sha-完整SHA`，证明绑定仓库/workflow/main push/run/完整SHA/digest/platform；串行锁中重新核对main才移动edge。正式版本不跟随edge。验证输出与候选证明来自当前run，不接受PR或源码中的同名JSON。
3. 在个人发布环境核对同SHA结果、审阅证据并明确授权后，才推送已有main提交对应的 `vX.Y.Z` tag。`docker-publish.yml`检查该tag属于main且有仍可取得的成功候选证明，跨package复制同digest，不重新构建或修改OCI label。手工重试入口仅在main执行，输入已有tag。
4. 正式包、源码tag和发布清单身份一致后创建/补齐draft；所有附件及其SHA-256匹配才公开Release。相同版本不同digest或附件拒绝覆盖；中断后只对同一身份重试。候选`mediaflow-dev`和正式`mediaflow`均保持public，发布前后校验可见性；GITHUB_TOKEN仅在对应job使用，Docker凭据存于runner临时目录并logout。

main推送这批工作流会启用候选写入，版本tag会触发正式提升与Release写入。仅授权本地实施时不要推送、创建远端tag或手动启动发布。真实发布前在个人环境确认Actions可写对应包、两包public可见性、Release权限及分支/标签治理；不要在公司设备登录或更改现有账号/remote/凭据。

GHCR新包默认private。首次创建前仅允许404进入上传；上传后若仍非public，流程保留已上传镜像并停止，不生成候选证明或正式发布包。管理员在个人环境打开对应包的Package settings，将可见性设为public，再重试同一发布身份；脚本不自动修改可见性。已有包非public会在写入前拒绝，API错误或未知可见性不会放行。包一旦public不能再转回private，镜像内容须适合公开分发。[GitHub包可见性说明](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)

若证明已过期/镜像缺失，重新运行可信main完整CI，不在正式发布流程临时构建。若重建同SHA产生不同digest，保持拒绝覆盖，调查差异并使用新的源码/版本身份。正式镜像及Release不自动清理。

## 完整发布包与信任

每个版本有 `release.json`、`compose.yaml`、`env.example`、`roots.example.json`、`install.py`、`release-notes.md`、`checksums.txt`。`env.example` 是安装器的 **JSON配置示例**，不是Compose dotenv文件；安装器生成私有 `.env`。

从指定仓库已公开的正式Release获取全部文件；核对tag源码、可信CI证明、完整SHA、平台及候选/正式digest。`checksums.txt`的SHA-256应从可信Release资产元数据或可信渠道独立取得；不能把同一未受信下载中自算的值当作来源证明。传输摘要不能替代来源核对。

执行下载的安装器前，在可信工作站先核对 `checksums.txt` 的独立摘要，再对其余文件运行 `sha256sum -c checksums.txt`（macOS可用相应SHA-256工具）。Python自检发生在代码执行之后，不能独立证明安装器本身可信。拒绝draft、prerelease、缺附件、摘要不符和不明仓库文件。

## NAS前置条件

- Linux amd64；Docker daemon与Compose v2支持 `up --wait`、`--pull never`、长bind的 `create_host_path:false` 和JSON config；NAS网络能拉取GHCR。只在目标NAS的本地Docker环境执行安装，不使用远端Docker context。
- 执行账户有Docker权限；runtime UID/GID为非root，安装账户为该属主或管理员。仅修改本次新建文件属主，不递归chown现有媒体。
- 三个独立、绝对、无符号链接/重叠/控制字符的路径：尚不存在的instance_dir，以及预建的incoming_dir、media_dir。父目录由管理员控制，incoming只读、media可写，彼此与正式实例/其他测试实例隔离。`/config`使用NAS本地兼容文件系统，不能放SMB/NFS；预留镜像与配置空间。
- 两个public镜像包可匿名拉取，NAS无需GitHub登录或read:packages PAT；发布凭据不复制到NAS。HTTPS Origin、代理来源精确CIDR、显式内网/回环监听地址及可用端口先确定；不开放默认路由代理信任。

## 检查、预览与安装

将完整且已核对的发布文件放入可信bundle目录，复制 `env.example` 为本地 `install.json` 并填入路径、UID/GID、Origin、CIDR和端口。安装配置不含PAT或引导密钥。以下CHECKSUM替换为前述独立取得的64位摘要：

```sh
python3 bundle/install.py check --bundle bundle --checksums-sha256 CHECKSUM --config install.json
python3 bundle/install.py plan --bundle bundle --checksums-sha256 CHECKSUM --config install.json
```

这两个命令只检查发布包与静态配置并输出预览，不调用Docker、不拉取、不创建实例；`runtime_preflight:not-run`是待执行。管理员核对预览后，在已明确授权的空白目标执行：

```sh
python3 bundle/install.py install --bundle bundle --checksums-sha256 CHECKSUM --config install.json
```

安装在独立实例锁下重做检查：Docker/Compose、已有Compose实例、digest与OCI身份、空间、实际UID挂载访问和端口，然后才创建实例。运行配置只采纳显式JSON与已核对模板，清除宿主MEDIAFLOW/COMPOSE覆盖；渲染挂载必须逐项一致。随机引导secret为0600，读写仅授权给runtime属主；secret值不出现在计划、命令参数或输出。

拉取等预检失败不会创建实例，保护诊断保存在输出指出的临时 `preflight.log`（0600，目录0700）；管理员保管所需证据后自行处理。创建实例后的失败保留 `deployment.json`、`install.log`、`failure.log`、config及secret；不自动down、删除数据库、重生成密钥或重装。不要将配置目录、secret或Docker认证文件上传为CI附件。

`ready-for-initialization`只表示容器健康与直接healthcheck通过，仍需管理员在实际HTTPS入口完成初始化、登录、REST/SSE及NAS挂载/重启验证；具体代理配置见 [反向代理规程](reverse-proxy.md)，已有实例的备份/恢复另见 [备份恢复规程](backup-restore.md)。安装器无升级或恢复入口。

## 必须保留的真实验收证据

在个人发布环境先完成同SHA CI、候选、同digest正式提升、重复发布和中断补齐；在隔离Linux Docker环境完成完整发布包的首次安装与拒绝重装、错误凭据/摘要/路径/空间/锁/健康超时及清理记录，再进行空白NAS首装。保留源码SHA、run、证明、digest、工具/平台、命令/退出、初始化/HTTPS/SSE和权限结果；不能将替身测试计入这些结果。失败仅保全现场，由管理员审阅后清理专用测试资源，不对正式实例执行清理命令。

首次版本没有支持的升级来源，`compatible_from:[]`、`recovery:manual-required`。没有真实证据的环节继续待验，不能凭工作流文件或本地回归宣布发布/安装验收通过。
