# M2 备份与干净恢复

MediaFlow 使用 SQLite WAL。不得在 Core 写入期间只复制 `mediaflow.db`；这会遗漏 WAL 或得到时间点不一致的副本。升级迁移前，Core 会通过 SQLite backup API 在 `/config/backups` 生成数据库、deployment-roots 和 manifest 三件套，并验证 SHA-256、`integrity_check`、外键与 schema 版本。

M3 启用加密 TMDB 配置后，数据库密文还绑定同一 `/config/instance.key`。迁移前自动三件套不包含该密钥，因此它只证明数据库和 deployment-roots 可恢复，不是完整的连接器凭据备份。需要保留 TMDB 配置时，离线备份必须把数据库、deployment-roots 与当时的 `instance.key` 作为同一恢复单元；密钥必须保持原始 32 字节、所有者、单硬链接和 `0600` 权限，且不得进入日志、工单、Git 或普通文档附件。

## 备份

升级前停止容器，保留当前 `/config` 的只读副本，再启动新镜像。离线复制必须包含整个 config 目录（数据库、可能存在的 `-wal`/`-shm`、deployment-roots、`instance.key` 和 backups），保留数字 UID/GID 与权限。复制后用固定镜像只读检查数据库：

```sh
puid=1000
pgid=1000
docker run --rm --user "${puid}:${pgid}" --read-only --tmpfs /tmp \
  --volume /path/to/offline-copy:/backup:ro \
  mediaflow:0.2.0-m2 verify-database --database /backup/mediaflow.db
```

只有命令返回 `integrity_check: ok`、空外键违规和预期关键表计数时，副本才可进入保留集。记录镜像 digest、schema 版本、manifest、散列、创建时间、NAS 文件系统和计数；不要备份 bootstrap secret。`instance.key` 的保管权限应至少等同管理员凭据，密钥散列只能进入受控清单，不能用散列替代密钥本体。

## 从迁移前备份干净恢复

恢复目标必须是新的空 `/config`，绝不能覆盖活动数据库。先停止当前服务并把原配置目录改为只读保留；创建一个新的空目录，设置与容器 PUID/PGID 一致的归属。将完整三件套目录只读挂载，并运行：

```sh
puid=1000
pgid=1000
docker run --rm --user "${puid}:${pgid}" --read-only --tmpfs /tmp \
  --volume /path/to/verified-backups:/backup:ro \
  --volume /path/to/new-empty-config:/restore \
  mediaflow:0.2.0-m2 restore-backup \
  --backup /backup/mediaflow-vN-TIMESTAMP.db \
  --config-dir /restore
```

`restore-backup` 会校验 manifest、数据库与 deployment-roots 散列，在暂存副本上再次执行 schema、`integrity_check` 和外键检查，最后无覆盖地发布 `mediaflow.db` 与 `deployment-roots.json`。任何失败都保持原 `/config` 不变；不要自动降级或把旧数据库复制回活动目录。

若备份数据库含 `connectors_integrations.secret_ciphertext`，在第一次启动恢复目录前，再把同一备份单元的 `instance.key` 无覆盖复制到新 `/config`，恢复原所有者和 `0600`，并确认它是常规单链接 32 字节文件。不要先启动 Core：缺少密钥时 Core 会创建新密钥，新密钥无法解密旧密文。若明确选择不恢复旧密钥，应先接受现有 TMDB 密文不可用，并在启动后删除/重新配置 integration；历史识别证据和决定不依赖 Token，不能通过复制另一个实例的密钥“修复”。

## 切换与核对

启动前再次把恢复出的 `deployment-roots.json` 与备份 manifest 配对核验。下面的 `backup_manifest` 必须是传给 `restore-backup` 的数据库同名 `.manifest.json`；命令比较的是 manifest 中记录的 deployment-roots SHA-256，而不是人工抄写值：

```sh
new_config=/path/to/new-empty-config
backup_manifest=/path/to/verified-backups/mediaflow-vN-TIMESTAMP.manifest.json
expected_roots_sha256=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["files"]["deployment_roots"]["sha256"])' "$backup_manifest")
actual_roots_sha256=$(sha256sum "$new_config/deployment-roots.json" | awk '{print $1}')
test "$expected_roots_sha256" = "$actual_roots_sha256"
```

随后同时更新 Compose `.env` 中的两项路径，禁止只切数据库而继续挂载旧根配置；`MEDIAFLOW_CONFIG_PATH` 指向的目录也必须包含与数据库配对的 `instance.key`：

```dotenv
MEDIAFLOW_CONFIG_PATH=/path/to/new-empty-config
MEDIAFLOW_ROOTS_CONFIG_PATH=/path/to/new-empty-config/deployment-roots.json
```

使用与备份 schema 兼容的固定镜像启动。核对 readiness、schema 版本、外键、管理员、收件目录和任务/文件关键计数，再切换反向代理流量。旧 `/config` 在验证完成前保持只读；确认后也按保留策略删除，不由恢复命令覆盖。

## 检查与证据边界

备份/恢复说明对应固定 `mediaflow:0.2.0-m2` 单镜像、单服务 Compose；仓库使用 Node 24.18.0、pnpm 11.10.0、Rust 1.97.0 的锁定检查组合。`just check-nas` 只验证本文包含干净恢复、`verify-database`、`restore-backup`、`integrity_check`、外键与关键计数说明；`just check-m2` 还会在静态检查后执行 Docker-backed 的 `just check-containers`、`just check-compose` 与 `just smoke-m2-compose`。这些命令在没有 Docker runtime 时必须失败，不能替代目标 UGREEN DXP 4800 Plus 上对新空 `/config` 的实际恢复演练。
