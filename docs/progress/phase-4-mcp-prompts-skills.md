# Phase 4: MCP、Prompts 和 Skills

> **Status**: Complete  
> **Tasks**: 5/5 complete  
> **Goal**: 完成 Claude Code、Codex、OpenCode 三个本地配置资源域，并保证从应用写入 WSL 后仍保留客户端未知字段。  
> **S.U.P.E.R Focus**: S、U、P

## Task Checklist

- [x] **P4-01**: 精简 MCP 模型和三端适配器
  - Priority: P0
  - Effort: L
  - Dependencies: P3-01、P3-02
  - Acceptance: MCP CRUD、按客户端启用/禁用和 live 写回在 Claude Code、Codex、OpenCode 三端通过；失败前验证和补偿保证无部分写入；未管理字段与客户端专属结构往返保留。
  - Notes: 将 `McpServer` 迁入无基础设施依赖的领域模块并只保留 Claude/Codex/OpenCode 三端状态，连接对象保持开放以无损承载客户端扩展；生产 DAO、旧 JSON 导入和启动空表判断全部切到 `core_mcp_servers`，旧 `mcp_servers` 仅保留为迁移源，非法领域记录和损坏 JSON 失败关闭。新增固定三端 MCP live 文件适配器，复用 WSL 路径、越界、symlink/reparse 与原子写防护；CRUD/启停按“完整预检→精确字节快照→live 写入→数据库提交”执行补偿事务，失败逆序恢复 live 且数据库不变，并以进程内写锁串行化。Claude JSON、Codex TOML 与 OpenCode JSON5 均保留根字段、单服务器未知字段和嵌套扩展；Codex 非法 TOML 不再静默删除成功。前端和 Tauri 只注册统一三端 MCP API，新增显式“同步到应用”入口，旧 Claude 专用和分应用 IPC 不再注册。领域 2/2、DAO 核心表合同 1/1、服务合同 5/5、命令 21/21、导入/投影 26/26、前端聚焦 22/22、前端全量 109 文件/730 项、Rust 库 2422 项（5 ignored）及全部 integration、typecheck/format/fmt/check、WSL/Windows 全目标 Clippy、diff-check 均通过；Windows 原生 EXE 在隔离 WSL UNC 根完成三端 CRUD/启停/手动同步/未知字段保留/跨端失败回滚 1/1，测试目录与 6.2 GB 临时 MSVC 缓存已清理。CI/夜间合同改为本地 TEMP 编译后直接以 UNC 环境启动测试 EXE，规避 `mt.exe` 的 `LNK1327`；S.U.P.E.R 10/10。

- [x] **P4-02**: 重构 Prompt 为三端独立版本库
  - Priority: P0
  - Effort: L
  - Dependencies: P3-01
  - Acceptance: Claude Code 的 `CLAUDE.md` 与 Codex/OpenCode 的 `AGENTS.md` 独立保存、启用和写回；每个客户端最多保留 20 个版本；外部修改可由后续统一扫描契约发现。
  - Notes: 将 Prompt 迁为 Claude Code、Codex、OpenCode 三端独立的 `core_prompt_versions` 版本库，固定 live 映射为 Claude `CLAUDE.md`、Codex/OpenCode `AGENTS.md`，并由领域校验、DAO 预分配和 SQLite 触发器共同执行每客户端/名称最多 20 版。每端只允许一个启用版本；新库使用按 `client_id` 的部分唯一索引，已有 v17 错误索引在迁移 savepoint 内保留最后更新的启用记录、仅停用重复项并原子修复索引，不删除版本或写 live。Prompt 服务以进程内锁串行化 CRUD、启用、首次导入、手动 live 导入和显式写回；普通启用/编辑检测外部修改并停止覆盖，写 live 后数据库失败会恢复精确原始字节。Tauri 命令仅接受三端并在未知客户端、路径 ID 不一致或非法记录时保持数据库和 live 零写；前端使用后端权威版本号，按当前客户端提供“从 live 导入”和确认后的“同步到 live”，并移除 PromptPanel 对 Deep Link/Profile 旧事件的监听。旧 JSON Prompt 自动导入和 `prompt_files.rs` 已删除。领域、DAO/schema、服务、命令和 Windows 合同均通过；Prompt 前端聚焦 47/47、前端全量 109 文件/735 项、Rust 库 2425 项（5 ignored）及全部 integration、typecheck/format/fmt/check、WSL/Windows 全目标 Clippy、diff-check 均通过；Windows 原生 EXE 在隔离 WSL UNC 根完成三端版本写入、外改阻断和精确回滚 1/1；S.U.P.E.R 10/10。

- [x] **P4-03**: 精简 Skill 本地核心和三端复制
  - Priority: P0
  - Effort: XL
  - Dependencies: P1-02、P3-01
  - Acceptance: Skill 只以本地目录为来源，能够在三个目标客户端之间显式复制和同步；所有路径经过固定 WSL 根目录、越界与链接防护；核心不依赖在线仓库、安装器或升级器。
  - Notes: 新增无基础设施依赖的 `LocalSkill` 领域模型、`LocalSkillRepository`/`LocalSkillTreePort` 端口、`core_skills` DAO 和固定三端 live 文件树适配器；Skill 内容仅存在于 Claude/Codex/OpenCode 的 `skills/<directory>` 普通目录，数据库只保存元数据、启用状态、大小、文件数和基线哈希，不建立独立 SSOT 或 symlink。导入要求显式 `sourceClient`，手动同步以来源只读、全部目标预检、再逐目标写入和数据库提交的单向流水线执行；外部修改、路径越界、symlink/reparse、非法 frontmatter、文件写失败、数据库保存/删除失败及无基线删除均有零写或精确逆序补偿合同，迁移记录可通过首次手动同步建立基线。前端提供按当前客户端显式同步并同时刷新受管/未受管列表；新核心不依赖旧在线仓库、安装器或升级器，旧链留待 P4-05 物理删除。服务单元合同 8/8、文件树适配器 1/1、集成合同 4/4、前端 Skill 聚焦 49/49、前端全量 109 文件/737 项、Rust 库 2436 项（5 ignored）及全部 integration、typecheck/format/fmt、WSL 全目标 Clippy、diff-check 均通过；Windows 原生 EXE 在真实 WSL UNC 路径完成三端普通文件复制、CRLF/嵌套/二进制保真、显式来源同步、最后目标外改时全局零写和 WSL link/reparse 拒绝 1/1；S.U.P.E.R 10/10。

- [x] **P4-04**: 实现 Skill 限额和忽略规则
  - Priority: P0
  - Effort: M
  - Dependencies: P4-03
  - Acceptance: 单文件、单 Skill 文件数、单 Skill 总量及云同步总量限制均有边界测试；扫描稳定忽略 `.git`、`node_modules`、构建输出、缓存和临时文件；超限 Skill 可继续本地使用但不可进入 WebDAV 同步记录。
  - Notes: 在纯领域层冻结 `10 MiB/单文件`、`500 文件/Skill`、`20 MiB/Skill` 和 `200 MiB/Skills 云同步总量`，所有边界均按“恰好上限允许、多 1 拒绝”测试；聚合校验只统计已具备单体资格的记录，在 P6 构建任何 WebDAV Skill 记录前可直接失败关闭。live 文件树按大小、实际文件数和最大文件计算 `cloud_eligible`，导入、手动同步及启用复制会持久化真实资格；超限目录仍可读取、导入、复制和本地使用，只是不进入云同步候选。扫描、哈希和复制按稳定的大小写不敏感 basename/pattern 忽略 `.git`、`node_modules`、常见 `target/dist/build/out/coverage` 构建输出、框架输出、缓存目录、`tmp/temp` 目录及 `.tmp/.temp/.swp/.swo/~` 等临时文件；同步采用受保护的 managed-entry 原地替换并保留目标已有忽略内容，显式删除仍递归检查包括忽略项在内的全部后代，拒绝 link/reparse 后再删除。领域合同 4/4、文件树适配器 2/2、本地服务集成 5/5、前端全量 109 文件/737 项、Rust 库 2439 项（5 ignored）及全部 integration、typecheck/format/fmt、WSL 与 Windows 全目标 Clippy、diff-check 均通过；Windows 原生 EXE 在真实 WSL UNC 验证忽略内容不复制、目标忽略内容保留、`10 MiB + 1 byte` 文件仍本地可用但云资格为假 1/1；临时 UNC 夹具和 Windows 源码镜像均已清理；S.U.P.E.R 10/10。

- [x] **P4-05**: 删除在线 Skill 仓库、安装和升级入口
  - Priority: P1
  - Effort: M
  - Dependencies: P4-03
  - Acceptance: 前端、Tauri 命令、服务、后台任务、设置、依赖和文案均无在线 Skill 仓库、下载、安装或升级入口；保留本地只读版本查询以及显式的官方升级命令展示能力。
  - Notes: 将生产 Skill 边界收敛为本地列表、扫描导入、三端启停、显式来源同步和从全部已启用客户端删除；前端移除仓库发现、skills.sh、在线安装/更新、ZIP 安装、备份恢复、存储位置/同步方式设置和 Skill Deep Link，统一面板只呈现本地普通文件树及“仅本地”云资格。后端物理删除旧 `SkillService`、Skill Deep Link 模块及所有在线/安装/更新 IPC 注册，保留的 `LocalSkillService` 不依赖下载器、仓库或升级器；v16 `skill_repos` 只作为迁移忽略计数和后续 schema 清场证据，旧 `skills.zip`/S3/WebDAV 整包协议按依赖留给 Phase 6/10 替换。迁移净化明确丢弃 `skillSyncMethod` 与 `skillStorageLocation`，四语文案和旧 MSW handler 同步删除。静态清场合同 2/2、迁移聚焦 8/8、前端聚焦 5 文件/30 项及全量 109 文件/704 项、Rust 库 2390 项（5 ignored）与全部 integration、typecheck/format/fmt/check/diff-check、WSL/Windows 全目标 Clippy 均通过；Windows 原生 EXE 在真实 WSL UNC 上复验 MCP、Prompt、Skill Phase 4 合同 3/3；S.U.P.E.R 10/10。

## Phase Gate

- [x] All tasks above are checked off.
- [x] MCP、Prompts、Skills 均只支持 Claude Code、Codex、OpenCode。
- [x] 三类资源都能从应用安全写入固定 WSL Ubuntu 路径。
- [x] 三端客户端未知字段和专属结构均能无损往返。
- [x] MCP 和 Skills 提供显式本地扫描/同步入口，不依赖后台自动覆盖。
- [x] Prompt 每端版本数不超过 20，Skill 所有限额和忽略规则有负面测试。
- [x] 在线 Skill 仓库、安装和升级实现已彻底移除。
- [x] Frontend、Rust、Windows 原生到 WSL UNC 阶段门全部通过。
- [x] S.U.P.E.R review is 10/10 for every completed task.

## Notes

- 2026-08-16 导入冲突回归修复：实机确认 44 个未管理目录中 25 个跨端重名，Windows `:Zone.Identifier` 元数据曾造成伪内容差异，另有真实不同的同名副本；扫描摘要现忽略该元数据。导入对话框默认只选择明确内容来源，切换来源时重置目标，避免隐式跨端覆盖；用户手动选中真实不同副本时仍保持全局零写，并改为准确提示“与所选来源内容不同”。前端 43 文件/218 项、Rust library 201 项（6 ignored）及全部 integration、严格 Clippy/格式/类型检查通过；`3.19.4` Windows 原生 EXE 对真实 44 个目录完成 44/44 来源单选 DOM 审计，未执行测试导入；S.U.P.E.R 10/10。
- 2026-08-16 回归修复：Skill 页恢复顶部和空状态“导入已有”入口，未管理 Skill 改为用户触发后扫描并立即显示进度；目录扫描、导入、同步、启停和卸载移入阻塞线程，列表/扫描失败显式呈现，单个非法 `SKILL.md` 只跳过自身。新增前端和 Rust 回归合同；`3.19.3` Windows 原生 EXE 在真实三端 WSL 目录中识别 44 个可导入 Skill，扫描期间界面保持响应且未执行测试导入。
- Phase 4 只建立三个资源域的本地核心和 live 适配器；统一 5 秒/30 秒扫描、自写抑制、冲突分类和冲突中心在 Phase 5 完成。
- WebDAV 记录模型、加密和多设备合并在 Phase 6 完成，本阶段不得为了云同步把凭据或原始会话纳入资源记录。
- 所有客户端范围、路径和写入纪律继承 Phase 1 与 Phase 3 已验证的固定 Windows/WSL 边界。
