# Phase 6: WebDAV Sync v3

> **Status**: Complete  
> **Tasks**: 9/9 complete  
> **Goal**: 实现仅由用户手动触发、逐记录、多设备双向、端到端加密且可安全处理并发的 WebDAV Sync v3。  
> **S.U.P.E.R Focus**: P、U、R

## Task Checklist

- [x] **P6-01**: 定义 sync-v3 记录、清单、设备和墓碑 schema
  - Priority: P0
  - Effort: L
  - Dependencies: P2-01、P5-04
  - Acceptance: schema 可版本化，稳定描述逐记录内容、版本、哈希、同步基线、设备确认代次和永久墓碑；显式排除当前供应商、固定路径、设备 ID、WebDAV 凭据、简报模型配置、运行状态、原始会话、搜索索引和恢复命令等设备级或禁止上云数据。
  - Notes: 新增纯领域 `sync_v3` schema：数字协议版本 v3、schema v1、严格设备/记录 ID、小写 SHA-256、七类可移植领域、规范 JSON/稳定哈希、live 与永久墓碑互斥、版本化基线、严格排序 manifest、设备登记/退役/确认代次及安全压缩判定。载荷通过领域顶层白名单和递归禁用字段拒绝当前供应商、设备设置、固定路径、WebDAV 凭据、简报模型、运行状态、原始会话、索引和恢复命令；核心不依赖 Tauri、HTTP、SQLite、文件系统或 Windows API。聚焦合同 9/9、串行 Rust library 2388 passed（5 ignored）、全部 integration、`cargo check --all-targets`、WSL/Windows 全目标 Clippy 通过；Windows 原生 schema 合同 8/8，随后只新增一个纯 JSON 精确夹具测试且产品代码未变化；S.U.P.E.R 10/10。

- [x] **P6-02**: 实现 Argon2id + AES-256-GCM envelope
  - Priority: P0
  - Effort: L
  - Dependencies: P2-04、P6-01
  - Acceptance: 口令派生参数、envelope 和 AAD 全部版本化；每个对象使用唯一 nonce；已知答案、往返、篡改、错误口令、错误对象类型/记录 ID/版本和零明文残留测试通过。
  - Notes: 新增独立的纯领域加密 envelope、稳定加密端口和可替换随机源适配器。固定 Argon2id v19 为 64 MiB、3 iterations、1 lane、32 字节输出，AES-256-GCM 使用每对象 12 字节随机 nonce 和 16 字节标签；KDF、envelope、AAD 均严格版本化，AAD 绑定协议、算法、完整 KDF profile、manifest/record 类型、记录 ID 和对象版本。主密钥每次同步会话只派生一次并由 `Zeroizing<[u8; 32]>` 托管，认证失败缓冲使用 `Zeroizing<Vec<u8>>` 且不返回部分明文；口令、主密钥和明文 Debug/序列化均脱敏。核心不依赖 Tauri、HTTP、SQLite、文件系统或 Windows API。已知答案、往返、唯一 nonce、错误口令、密文/nonce/KDF salt/身份元数据篡改、错误对象类型/记录 ID/版本、未知版本/字段和口令边界合同 WSL 6/6、Windows 原生 6/6；串行 Rust library 2388 passed（5 ignored）及全部 integration、`cargo check --all-targets`、fmt、WSL/Windows 全目标 Clippy、diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P6-03**: 精简 WebDAV 传输层并支持 ETag/`If-Match`
  - Priority: P0
  - Effort: L
  - Dependencies: P6-01
  - Acceptance: 传输端口只提供 sync-v3 所需读取、条件写和必要目录操作；可靠映射 ETag、`If-Match`、认证、超限、超时和 HTTP 错误，URL 与诊断不泄露凭据或查询参数。
  - Notes: 新增纯领域 `SyncRemotePath`、严格 opaque ETag、CAS 条件、限额远端对象和写回收据，独立异步 `SyncTransportPort` 只暴露必要目录创建、限额读取和条件写三项能力；新 reqwest 适配器拥有独立直连客户端，不依赖旧 `proxy::http_client`、整包 WebDAV、Tauri、SQLite 或日志模块。读取同时执行 `Content-Length` 与流式上限校验，写入固定 24 MiB 绝对上限；更新使用原样 `If-Match`，首次创建使用 `If-None-Match: *`，成功响应严格解析可选 ETag。Basic Auth 只在请求边界注入，密码由 `Zeroizing<String>` 托管；配置 Debug、稳定错误码和上下文均不包含 URL、查询参数、凭据、远端响应正文或底层 reqwest 文本。认证、412 CAS、容量、HTTP 408/504、真实客户端 deadline、连接/传输、非法响应和状态码分别映射，目录已存在时以 `PROPFIND Depth: 0` 验证。合同 WSL 8/8、Windows 原生本地 HTTP 8/8；串行 Rust library 2388 passed（5 ignored）及全部 integration、fmt、`cargo check --all-targets`、WSL/Windows 全目标 Clippy、diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P6-04**: 实现逐记录三方合并
  - Priority: P0
  - Effort: XL
  - Dependencies: P6-01
  - Acceptance: 基线、本地、远端逐记录合并矩阵覆盖新增、单边修改、同值修改、单边删除、更新/删除、双边不同修改和无基线情形；无冲突记录继续，冲突记录两端保持原状。
  - Notes: 新增独立纯领域 `sync_merge`，以严格排序的基线、本地和远端逐记录输入产出唯一合并记录、显式本地/远端 `Unchanged`/`ApplyMerged` 动作和内容为空的冲突摘要，不执行文件、数据库或网络写入。无基线单侧新增会向另一侧传播；同值新增、同值并发修改和双端删除按内容哈希/删除状态确定性收敛，其中双端墓碑保留更保守的较高引入代次；单边修改和永久墓碑删除只更新未变化一侧。双边不同修改、更新/删除和基线记录凭空缺失只隔离目标 ID，干净记录继续生成动作，冲突摘要仅含状态、修订和哈希。输入记录、hash、严格 ID 顺序、重复 ID、同设备过期计数及同一设备/计数复用不同内容均在任何合并结果前整体失败关闭。矩阵合同 WSL 6/6、Windows 原生 6/6；相邻 schema 9/9、crypto 6/6、transport 8/8；串行 Rust library 2388 passed（5 ignored）及全部 integration、fmt、`cargo check --all-targets`、WSL/Windows 全目标 Clippy、diff-check 均通过；临时 Windows 镜像已删除；S.U.P.E.R 10/10。

- [x] **P6-05**: 实现设备登记、首次预览和退役
  - Priority: P0
  - Effort: L
  - Dependencies: P6-04
  - Acceptance: 新设备首次同步先只读展示新增、修改、删除和冲突计数，未确认前本地和远端零写；确认后登记固定设备身份；退役操作显式提示旧设备重新出现的风险。
  - Notes: 新增独立纯领域 `sync_device` 生命周期核心，同时支持空远端首次创建和加入已有 sync-v3。首次预览只返回新增、修改、删除和冲突计数，令牌绑定候选设备身份、本地记录/基线指纹、远端 manifest 哈希与 generation、ETag、预览时间和计数；确认时重算令牌，因此候选设备、本地状态或远端状态变化均失败关闭，确认前不产生身份、本地或远端写入。确认后只返回固定设备身份、manifest 登记、`CreateOnly`/`Match` 远端 guard 和 P6-04 合并应用计划；已有固定身份不可被替换。退役使用绑定目标设备的显式风险确认，禁止当前 writer 自退役，只生成设备表变更计划，不提前压缩墓碑。重复设备 ID、非法名称、时间回退和未知 revision owner 均失败关闭；预览与诊断不含 record ID、payload 或敏感配置。合同 WSL/Windows 原生 9/9，相邻 schema 9/9、crypto 6/6、merge 6/6、transport 8/8；串行 Rust library 2388 passed（5 ignored）及全部 integration、`cargo check --all-targets`、fmt、WSL/Windows 全目标 Clippy、diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P6-06**: 实现永久墓碑与安全压缩条件
  - Priority: P0
  - Effort: M
  - Dependencies: P6-05
  - Acceptance: 删除通过永久墓碑传播；存在未追平的有效设备时禁止压缩；仅在所有有效设备确认达到对应版本后允许明确压缩，退役设备按已确认状态参与判定。
  - Notes: 新增独立纯领域 `sync_tombstone`，压缩只生成显式、全有或全无的零写计划，不执行 WebDAV、文件或数据库操作。输入必须提供与 manifest 索引完全匹配的完整记录快照，以及严格排序、无重复且仅指向墓碑的明确选择；未来代次墓碑、live 记录、缺失/重复记录和 manifest 不一致均失败关闭。每个选中墓碑要求所有 Active 设备的 `acknowledged_generation` 达到其引入代次；正式 Retired 设备不再阻塞，但以排除计数保留在判定摘要中。成功计划绑定当前 generation/writer，给出下一 generation、待移除 ID 和保持严格顺序的剩余索引，实际远端删除与 CAS 留给后续应用层。新增合同 4/4，相邻 schema 9/9、device 9/9、merge 6/6，共 28/28；Rust library Clippy、fmt、diff-check 通过。按核心功能优先，本任务未重复执行 Windows 原生与全量回归，留待 Phase 6 阶段门统一验证；S.U.P.E.R 10/10。

- [x] **P6-07**: 实现一次 CAS 重合并和失败停止
  - Priority: P0
  - Effort: M
  - Dependencies: P6-03、P6-04
  - Acceptance: 首次 `If-Match` 竞争失败后只重新拉取并合并一次；第二次竞争立即停止并保持本地/远端零覆盖，不进入无限重试或后台重试。
  - Notes: 新增纯领域 `sync_cas` 状态机，将首次条件写、一次 fresh refetch/remerge 和最终停止建模为不可绕过的类型。首次 `PreconditionFailed` 只返回绑定原 generation、manifest hash 与 ETag 的一次重拉令牌；重合并必须证明 generation 增长且 manifest/ETag 均变化，缓存或部分变化的观察失败关闭。第二次 `PreconditionFailed` 和所有非 CAS 传输失败只返回不含 merge batch 的停止结果；本地应用计划仅在远端提交成功后释放。远端记录快照必须与 manifest 精确匹配。定向合同 7/7 和 Rust 编译通过；按核心优先停止了会重新编译全部桌面依赖的 library Clippy，留待最终构建前统一检查；S.U.P.E.R 10/10。

- [x] **P6-08**: 接入冲突中心和临时回滚点
  - Priority: P0
  - Effort: L
  - Dependencies: P5-05、P6-04
  - Acceptance: WebDAV 冲突通过既有统一来源端口进入冲突中心；无冲突记录照常完成；任何本地高风险应用前创建 DPAPI 临时回滚点，成功删除、失败最多保留 3 份。
  - Notes: 新增 `WebDavConflictSource`，将真实 `SyncMergeConflict` 映射为既有统一冲突源条目，仅暴露领域、记录键、冲突类型、修改时间和内容哈希，不携带 payload；干净的 resolved 记录仍保留在 merge batch 中继续应用。WebDAV 记录键改用 `PortableRecordId` 规则验证，不再误套本地扫描 ID。新增 `SyncLocalApplyPort` 与 `apply_committed_sync_batch`，仅在存在本地应用动作时先创建 `RollbackPointPurpose::WebdavSync` 加密临时回滚点，成功删除，失败调用既有保留/最多 3 份裁剪生命周期；没有本地动作时零回滚写入。定向合同 3/3 和 Rust 编译通过；更广回归按核心优先留到最终构建前；S.U.P.E.R 10/10。

- [x] **P6-09**: 删除 WebDAV 自动同步和 S3
  - Priority: P0
  - Effort: M
  - Dependencies: P6-08
  - Acceptance: 旧整库 SQL/ZIP 协议、WebDAV 自动同步、S3 服务、命令、设置、后台任务、依赖、UI、测试和文档入口彻底移除；产品只在用户点击“立即同步”时访问 WebDAV，失败后不后台重试。
  - Notes: 物理删除旧 WebDAV 整库归档、自动同步 worker、S3 服务/命令/设置、同步协议模块及直接 `zip 2.x` 依赖；数据库备份层不再暴露 sync 专用 SQL 导入/导出，前端移除自动同步、S3、上传/下载、确认框、后台事件和 API，仅保留 WebDAV 凭据保存、显式只读连接测试及独立 sync-v3 传输适配器。代表性迁移继续读取旧设置但只输出 `baseUrl`、`username`、`remoteRoot`、`profile`，密码不序列化，旧状态/S3 字段被剥离。三语用户手册删除 v2 整包、自动同步和覆盖式上传/下载说明，并由静态合同防回归。聚焦证据：删除表面 2/2、sync-v3 WebDAV 传输 9/9、命令密码语义 2/2、设置/凭据边界 13/13、retained migration 8/8、数据库备份 11/11、WebDAV 组件 5/5，`pnpm typecheck`、目标文件 Prettier 与 Rust fmt 通过。更广 App 集成在 WSL 挂载盘出现 7 个既有 5/10 秒超时，未计为通过；依照后续计划的逐任务聚焦规则，完整前端/Rust/Windows 阶段门留到 M1-04。S.U.P.E.R 10/10。

## Phase Gate

- [x] All tasks above are checked off.
- [x] sync-v3 schema 全部版本化，且设备级数据、凭据和原始会话无法进入载荷。
- [x] Argon2id、AES-256-GCM、唯一 nonce、AAD、篡改和错误口令合同通过，远端无明文。
- [x] WebDAV 仅手动触发，ETag/`If-Match` 条件写及错误映射可靠。
- [x] 双设备和三设备逐记录合并矩阵覆盖新增、修改、删除和并发冲突。
- [x] 新设备首次预览未确认前保持本地与远端零写。
- [x] 永久墓碑、设备登记/退役和安全压缩条件全部通过。
- [x] 首次 CAS 失败只重合并一次，第二次竞争停止且零覆盖。
- [x] WebDAV 冲突不阻塞无冲突记录，并复用统一冲突中心与临时回滚点。
- [x] 旧整包 WebDAV、自动同步和 S3 的活动代码与产品入口已彻底删除。
- [x] Frontend、Rust、Windows 原生网络/凭据/加密阶段门全部通过。
- [x] S.U.P.E.R review is 10/10 for every completed task.

## Notes

- 2026-08-16 完成手动设置页、应用编排、设备管理和自动化双运行时收敛合同。真实 WebDAV 与两台实体设备的端到端烟测属于最终外部环境验收，不伪装为自动化测试结论。
- M1-02 follow-up 已新增独立 `SyncV3Orchestrator`：完整读取并解密远端 manifest/逐记录密文，严格校验 manifest 索引，执行三方合并，以内容哈希命名不可变加密记录对象，最后用 `If-Match` 提交新 manifest。首次 CAS 竞争只允许重新读取并合并一次，第二次竞争停止；只有远端 manifest 提交成功后才发布冲突并携带已提交 generation 执行本地应用/基线更新。错误口令、合法 envelope 内密文篡改和非法 manifest 均在任何写入前停止。编排、CAS、冲突、加密、合并、schema 和 WebDAV 传输合同、Windows 原生构建与完整自动化阶段门均已通过。
- WebDAV 同步只能由用户明确点击触发；不得定时访问、后台重试或恢复自动同步。
- 同步协议按记录工作，不得复用 `db.sql + skills.zip`、数据库导出或整包覆盖语义。
- 同步口令、WebDAV 密码和设备级设置只保存在本机受保护边界，不写入 sync-v3 对象。
- 当前供应商、固定 WSL 路径、设备 ID、WebDAV 凭据、简报模型配置和运行状态不跨设备同步。
- 原始会话、会话索引和恢复命令永不上云；完整且通过完整性验证的每日简报在 Phase 8 才接入 sync-v3。
- P6-01 只冻结纯领域 schema 和排除边界，不提前实现加密、HTTP、SQLite 或 UI。
