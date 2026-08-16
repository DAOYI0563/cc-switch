# Phase 3: 三端供应商核心

**Goal**: 完成 Claude Code、Codex、OpenCode 原生直连供应商管理。  
**Status**: Complete

## Tasks

- [x] **P3-01**: 将应用能力模型收敛为三端
  - Priority: P0
  - Effort: L
  - Dependencies: P2-01
  - Acceptance: 核心枚举不再包含非目标客户端。
  - Notes: 新领域、数据库约束、生产命令解析、启动扫描、供应商批量 live 同步、MCP/Prompt/Skill、托盘和会话入口均收敛为 Claude/Codex/OpenCode；遗留客户端字段只允许兼容读取，不再默认创建、重新序列化或写入 live。修复运行时 SQLite authorizer 下旧 JSON 供应商 `INSERT OR REPLACE` 隐式触发已删除 `provider_health` 外键级联的问题，改为不删除父行的 UPSERT 并验证旧子记录零写。三端边界 9/9、非目标 Prompt/数据库合同 2/2、应用类型 2/2、`import_export_sync` 26/26、前端 108 文件/734 项、Rust library 2418 项及完整集成测试、fmt/check/clippy/diff-check 全部通过；S.U.P.E.R 10/10。

- [x] **P3-02**: 拆分三端 live 配置适配器
  - Priority: P0
  - Effort: XL
  - Dependencies: P1-03、P3-01
  - Acceptance: 各适配器可独立执行往返测试。
  - Notes: 定义了结构化 `LiveProviderConfigPort`、三端快照/记录/错误合同，并实现 Claude、Codex、OpenCode 三个独立适配器；普通供应商读取、写入、存在性检查、删除和导入均先收敛到端口。Claude 保留未知字段并剥离内部私有字段；OpenCode 以 additive JSONC 方式保留非供应商字段；Codex 保留 TOML 注释、未知字段、官方认证和模型目录，并将 catalog、`auth.json`、`config.toml` 纳入同一回滚边界。最终旁路审计又将官方切换后的陈旧第三方 `auth.json` 条件清理移入 Codex 适配器，由“旧供应商已成功回填”显式授权，晚期配置写失败会恢复已删除认证。适配器 12/12、供应商服务 29/29、供应商命令 10/10、Rust library 2426 项及完整集成测试、fmt/check/clippy/diff-check 全部通过；S.U.P.E.R 10/10。

- [x] **P3-03**: 实现三端原生供应商 CRUD/生效
  - Priority: P0
  - Effort: L
  - Dependencies: P3-02
  - Acceptance: Claude/Codex 切换与 OpenCode additive provider 写入均通过真实 WSL live 配置往返。
  - Notes: 新增三端生产专用 managed CRUD/切换用例，统一规范为 `official/custom`，OpenCode 强制 `custom` 且拒绝伪官方、OMO、代理认证、托管 OAuth 和 Claude 非原生协议；新增、编辑、删除、移除 live 和切换均有数据库、设备当前状态及 live 配置失败补偿，回滚失败会合并报告。生产 Tauri 命令、托盘和供应商终端入口均接入 managed 用例，并以静态边界测试禁止回退到旧 `list/switch_direct`。失败零残留/恢复和 managed 边界 31/31、官方种子 1/1、供应商命令 10/10、供应商服务 29/29、Rust library 2433 项（5 ignored）及全部 integration、`cargo check --tests`、全目标 clippy、fmt/diff-check 均通过；Windows 原生 Rust 二进制在 `\\wsl.localhost\Ubuntu\tmp\wsl-code-switch-p3-03-native` 完成 Claude/Codex 切换与 OpenCode additive CRUD 往返 1/1；S.U.P.E.R 10/10。

- [x] **P3-04**: 落实 Claude 特殊写入规则
  - Priority: P0
  - Effort: M
  - Dependencies: P3-02
  - Acceptance: `primaryApiKey` 与 `.claude.json` 行为正确。
  - Notes: Claude Remote WSL 插件配置固定经受保护的 `.claude/config.json` 端口原子写入：自定义供应商设置 `primaryApiKey: "any"`，官方供应商仅移除该字段，未知字段原样保留，非法 JSON/非对象根节点失败零写。managed 新增首个供应商、编辑当前供应商和显式切换共享同一规则入口；live 写入失败时恢复插件文件原始字节，前端切换不再二次写入。onboarding 固定经 home 级 `.claude.json` 文件 scope 设置/清除 `hasCompletedOnboarding`，保留 MCP 和未知字段。前端聚焦 32/32、Rust plugin/onboarding/managed 聚焦 5/5、Rust library 2439 项（5 ignored）及全部 integration、typecheck/format/fmt/check/全目标 clippy/diff-check 均通过；Windows 原生 Rust 二进制在 WSL UNC 完成特殊写入往返并确认零测试残留 1/1，Windows 全目标 clippy 通过；S.U.P.E.R 10/10。

- [x] **P3-05**: 落实 Codex `auth.json` 保留规则
  - Priority: P0
  - Effort: M
  - Dependencies: P3-02
  - Acceptance: 自定义供应商切换不破坏官方登录。
  - Notes: 将 Codex 官方认证保留从可关闭设置提升为固定 live 写入策略，删除 Rust/TypeScript 活动设置、UI 开关和四语文案；v16 旧字段仅作为迁移输入并被显式丢弃。自定义供应商新增、编辑当前供应商和显式切换只把第三方 key 投影到 `config.toml`，始终按原字节保留 `auth.json`；切回官方仅删除能够严格证明为旧第三方 API-key 残留的已知字段组合，OAuth、PAT、未知顶层字段和未知 token 字段全部 fail closed 保留。provider backfill 不持久化官方 tokens，catalog/auth/config 晚期写失败恢复原字节。Codex adapter 7/7、managed 合同 1/1、provider service 16/16、迁移与 proxy 兼容聚焦合同全部通过；前端 108 文件/731 项、Rust library 2441 项（5 ignored）及全部 integration、typecheck/format/fmt/check/WSL 与 Windows 全目标 clippy/diff-check 均通过；Windows 原生二进制在 WSL UNC 完成新增、编辑、切换、切回官方往返 1/1，`.codex` 与 `.wsl-code-switch` 零产品残留；S.U.P.E.R 10/10。

- [x] **P3-06**: 保留余额、订阅额度和自定义脚本
  - Priority: P1
  - Effort: L
  - Dependencies: P3-03
  - Acceptance: 无代理统计依赖且刷新行为正确。
  - Notes: 将余额、Coding Plan、Claude/Codex 官方订阅和通用 JavaScript 查询统一分派到三端 provider usage 用例，非目标客户端、OpenCode 官方订阅、Copilot、托管 OAuth 和未知模板均在触网前拒绝。新增独立直连 provider HTTP 适配器，所有保留额度链不再引用或初始化 `proxy::http_client`；删除 Rust `UsageCache`、额度事件、托盘副作用和前端缓存桥，并把 provider usage API/query key 从统计模块中拆出。自动刷新 `0` 表示关闭、正值按分钟；Claude/Codex 仅当前供应商刷新，OpenCode 仅 live 配置内供应商刷新，瞬时失败保留最后一次成功值。脚本 CPU/内存/栈与 2-30 秒 HTTP 超时边界保留，自定义模板继续允许显式 HTTP LAN。后端边界 5/5、脚本 4/4、余额 3/3、三端分派 8/8、Coding Plan 41/41、订阅 1/1、前端聚焦 50/50；前端全量 108 文件/732 项、Rust library 2444 项（5 ignored）及全部 integration、typecheck/format/fmt/check/WSL 与 Windows 全目标 clippy/diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P3-07**: 实现手动连通性和 `/models` 获取
  - Priority: P1
  - Effort: M
  - Dependencies: P3-03
  - Acceptance: 只检测当前选择目标，不形成测速系统。
  - Notes: 将连通性收敛为单个显式 `appId + providerId` 命令，只读取一个已保存的三端自定义供应商并通过独立直连 HTTP 适配器发送一次 GET；官方供应商、空 URL、非 HTTP(S) URL 和嵌入凭据均在触网前拒绝，任意 HTTP 响应均表示目标可达。`/models` 获取优先使用显式 `modelsUrl`，否则从当前 Base URL 唯一派生一个地址，不再维护候选列表、404 回退或重试；结果按 ID 排序去重并仅在真实新增/编辑表单中由用户点击后只读显示，不自动修改配置。删除活动批量测速、测速配置/历史、端点 CRUD、设置面板和供应商表单测速组件，并在新增/编辑时剥离旧测速元数据，v16 兼容读取留到 Phase 10 物理清除。后端边界 1/1、连通性 3/3、模型获取 4/4、前端聚焦 11/11；前端全量 111 文件/735 项、Rust library 2414 项（5 ignored）及全部 integration、typecheck/format/fmt/check/WSL 与 Windows 全目标 clippy/diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P3-08**: 实现 Claude/Codex 通用配置片段
  - Priority: P1
  - Effort: L
  - Dependencies: P3-02
  - Acceptance: 提取和写入会排除所有禁止字段。
  - Notes: 新增统一纯领域过滤策略，Claude JSON 与 Codex TOML 均递归排除凭据、MCP、Prompt、Skill、供应商模型/端点/路由/header 等专属字段，OpenCode 明确拒绝；手工非法片段在任何写入前失败，历史脏片段读取及 live 应用时静默净化。真实供应商表单支持独立编辑、从 WSL live 显式提取和按供应商启用，OpenCode 不显示入口；删除旧的 Claude/Codex 分叉编辑链。保存副作用收敛到 `CommonSnippetService`，片段与显式清空标记同事务更新，live 写失败会恢复旧数据库状态及历史供应商归一化，MCP 后置投影失败降级为可自愈警告。领域/主库聚焦 20/20、供应商集成 9/9、新服务负面及往返 4/4、前端 109 文件/729 项、Rust library 2417 项（5 ignored）及全部 integration、typecheck/format/fmt/check/WSL 与 Windows 全目标 clippy/diff-check 均通过；Windows 原生测试进程在 `\\wsl.localhost\Ubuntu\tmp\wsl-code-switch-p3-08-native` 完成 Claude/Codex 提取、启用、切换与失败回滚 4/4，测试目录已清理；S.U.P.E.R 10/10。

## Phase Completion Checklist

- [x] All tasks above are checked off.
- [x] Core client capability model contains only Claude Code, Codex, and OpenCode.
- [x] Each target client has an independently testable live-config adapter.
- [x] Claude/Codex official/custom switching and OpenCode custom additive provider CRUD pass real WSL roundtrips without a fake official OpenCode seed.
- [x] Claude `primaryApiKey` and onboarding rules are verified.
- [x] Codex official `auth.json` material survives custom provider switches.
- [x] Quota and custom usage queries have no proxy-statistics dependency.
- [x] Connectivity and `/models` checks remain explicit, single-target operations.
- [x] Common snippets exclude credentials and all managed resource domains.
