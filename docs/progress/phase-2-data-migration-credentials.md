# Phase 2: 精简数据层、迁移和凭据底座

**Goal**: 安全迁移保留数据，建立敏感信息存储。  
**Status**: Complete

## Tasks

- [x] **P2-01**: 设计新数据库 schema 和可逆迁移步骤
  - Priority: P0
  - Effort: L
  - Dependencies: P0-04
  - Acceptance: schema 只服务保留域且迁移步骤可测试。
  - Notes: schema 版本升至 v17，在独立 `core_schema` 模块建立 11 个 retained-domain shadow tables，约束客户端仅 Claude/Codex/OpenCode、供应商仅 official/custom、Prompt 每客户端/名称最多 20 版，并覆盖冲突、sync-v3、设备、简报、检查点和设置。v16 旧表保持不变，按 8 步可逆计划逐域迁移，当前只执行创建和结构校验。初始化已修正为任何 schema 写入前先创建升级保护副本；磁盘合同证明主库升到 v17，而保护副本仍为原始 v16 且无 `core_*` 表。schema 9/9、database 72/72（2 ignored）、fmt/check/clippy 通过。

- [x] **P2-02**: 实现旧 `.cc-switch` 只读识别
  - Priority: P0
  - Effort: M
  - Dependencies: P2-01
  - Acceptance: 不修改旧目录即可生成迁移预览。
  - Notes: 新增纯领域 `LegacyMigrationPreview`、`LegacyDataSource` 端口及固定 `%USERPROFILE%\.cc-switch` 适配器；SQLite 以 `READ_ONLY + URI immutable + query_only` 打开，JSON 直接解析 `serde_json::Value`，不复用会自动保存的 `MultiAppConfig::load()`。预览只返回来源版本、保留/忽略计数、已知文件 SHA-256 和目录指纹，不返回配置正文或凭据；数据库优先于残留 JSON。缺失/空目录、v1/v2 JSON、v16 DB、损坏/未来版本、非空 WAL、symlink 和 Windows junction 均有失败关闭合同；调用前后文件清单、字节和 mtime 完全一致，无 WAL/SHM/journal 或临时文件产生。WSL focused 10/10、Windows 原生 10/10、串行完整库 2382/2382（5 ignored）、database 18/18、fmt/clippy/diff-check 通过。

- [x] **P2-03**: 实现临时 DPAPI 加密回滚点
  - Priority: P0
  - Effort: L
  - Dependencies: P1-04
  - Acceptance: 成功删除、失败最多保留 3 份。
  - Notes: 新增纯领域 `RollbackPointMetadata`、用途/状态枚举和独立 `LocalProtector`、`TemporaryRollbackStore` 端口。Windows 适配器使用当前用户 `CryptProtectData`/`CryptUnprotectData`、应用固定 entropy 和 `CRYPTPROTECT_UI_FORBIDDEN`，密文不能跨用途恢复；临时存储使用版本化双层 envelope，外层验证密文 SHA-256，DPAPI 解密后再验证元数据、ID、载荷大小和载荷 SHA-256。成功立即删除，失败转保留态并只保留最新 3 份；非法 ID、篡改、错误 entropy/key、symlink、Windows junction、保护/写入失败均失败关闭且不产生部分文件。WSL 聚焦合同 11/11、Windows 原生真实 DPAPI 和存储合同 11/11、串行完整库 2394/2394（5 ignored）、WSL/Windows clippy `-D warnings`、fmt/diff-check 通过。

- [x] **P2-04**: 实现 Windows 凭据管理器 `SecretStore`
  - Priority: P0
  - Effort: L
  - Dependencies: P1-04
  - Acceptance: WebDAV 和简报密钥不进入设置文件。
  - Notes: 新增固定三类设备凭据的 `SecretStore` 端口和 Windows Generic Credential 适配器，target names 固定在 `com.zhldm.wsl-code-switch/*`，使用当前 Windows 用户的 `CRED_PERSIST_LOCAL_MACHINE` 凭据。WebDAV 旧明文密码仍可一次性反序列化迁移，但所有新设置序列化均省略密码；读取请求时按需注入，明确清空时删除凭据。凭据与设置文件采用事务式双写，设置写失败会精确恢复“未修改/原先不存在/原有值”三态，且不会误删未修改凭据。Windows 原生真实 Credential Manager 3/3、设置安全链 15/15、WebDAV 链 8/8；WSL 聚焦 2/2、13/13、8/8；串行完整库 2398/2398（5 ignored）；Windows/WSL clippy `-D warnings`、fmt 和测试凭据零残留均通过。

- [x] **P2-05**: 实现保留数据迁移及完整性校验
  - Priority: P0
  - Effort: L
  - Dependencies: P2-02、P2-03、P2-04
  - Acceptance: v16 样本迁移不丢目标记录，失败可恢复。
  - Notes: 新增代表性 v16 SQLite/`settings.json` retained-domain 迁移编排，在任何目标写入前创建 DPAPI 临时回滚点，并跨数据库、设备设置和 Windows Credential Manager 执行恢复。目标设置以现有合法设备字段为基底，旧设置只补充白名单缺项；代理、统计、Profiles、S3、目录覆盖、非目标客户端和旧迁移字段被清除，WebDAV `autoSync` 固定关闭，当前目标 WebDAV 配置及已有凭据优先，所有嵌套明文凭据均排除。数据库写入后重读、核对记录数和 SHA-256；数据库迁移标记与跨资源完成标记使用版本化结构并校验来源、内容哈希、时间和实际 committed content，损坏、孤立或异源标记均失败关闭。同源重试幂等，不同源或非空目标拒绝覆盖；成功后回滚点删除失败会明确报错，下一次启动仅重试清理且不重复迁移。WSL retained 8/8、database marker 7/7、完整库 2418/2418（5 ignored）、`clippy --lib --tests -D warnings`、fmt/check/diff-check 通过；Windows 原生 retained 9/9（含真实 DPAPI、Credential Manager 和精确文件恢复）、legacy 13/13、database marker 7/7、生产库 Clippy 通过，测试凭据零残留。

- [x] **P2-06**: 停止旧表活动读写，暂缓物理 DROP
  - Priority: P1
  - Effort: M
  - Dependencies: P2-05
  - Acceptance: 新核心运行不访问被删除功能表。
  - Notes: 在生产 SQLite 连接完成 schema 创建、v16→v17 迁移和 auto-vacuum 处理后安装 authorizer，拒绝 `proxy_config`、`proxy_request_logs`、`provider_health`、`proxy_live_backup`、`model_pricing`、`usage_daily_rollups`、`session_log_sync`、`stream_check_logs`、`profiles` 的 READ/INSERT/UPDATE/DELETE，同时保留 retained legacy 表和 `core_*` 表供后续迁移。生产启动不再创建代理/OAuth/定价/用量/自动同步/周期备份后台活动，旧整库导入导出、手工数据库快照、WebDAV 整库传输和全部 S3 IPC 已撤下；托盘活动路径只保留打开、三端供应商直连切换和退出。静态合同分别解析 setup 与真实 `generate_handler!` 注册段，避免禁止 IPC 因截断产生假阳性。WSL 完整库 2421/2421（5 ignored）、Clippy `--lib --tests -D warnings`、fmt/diff-check 通过；Windows 原生 authorizer 2/2、生产数据库初始化 1/1、迁移前保护 1/1、retained migration 9/9、启动 IPC 1/1、托盘 1/1、生产库 Clippy 通过。旧表本阶段未物理 DROP。

## Phase Completion Checklist

- [x] All tasks above are checked off.
- [x] Retained-domain schema is isolated and constrained to the target product.
- [x] v16-to-v17 schema migration is transactional and preserves legacy sources.
- [x] Legacy `.cc-switch` preview is read-only.
- [x] DPAPI rollback points pass success, failure, and retention contracts.
- [x] Windows Credential Manager secrets never enter settings or sync payloads.
- [x] Retained records migrate with integrity validation.
- [x] Deleted-domain tables have no active readers or writers.
