# Phase 5: 本地扫描和冲突中心

> **Status**: Complete — Skill 本地索引刷新增量已验证<br>
> **Tasks**: 5/5 基线完成；2026-08-17 Skill 增量阶段门已关闭<br>
> **Goal**: 周期、启动、恢复和页面进入只读感知固定 WSL 配置外改；仅在用户点击 Skill“导入已有”后显式刷新 managed 数据库索引，任何扫描或索引刷新都不写 WSL。<br>
> **S.U.P.E.R Focus**: U、P、R

## Task Checklist

P5-01 至 P5-05 的 Notes 保留原阶段历史证据；本页末尾的 Skill 增量阶段门记录 2026-08-17 最终工作树的重新验证结果。

- [x] **P5-01**: 定义统一扫描摘要和变化事件
  - Priority: P0
  - Effort: M
  - Dependencies: P4-01、P4-02、P4-03
  - Acceptance: 供应商、MCP、Prompts、Skills 共享稳定、可序列化、无敏感内容的扫描摘要与变化事件契约，但各自保留独立解析器和领域归一化逻辑；纯领域测试覆盖新增、修改、删除、无变化、读取错误和稳定排序。
  - Notes: 新增无基础设施依赖的 `domain/local_scan.rs`，以 `LocalScanTarget` 同时标识 Provider、MCP、Prompt、Skill 和 Claude/Codex/OpenCode 客户端；`LocalScanSummary` 对完整 live 表示保存规范化小写 SHA-256，对记录摘要按逻辑 ID 稳定排序并拒绝重复 ID、非法摘要、负时间及错误 schema。纯比较器在 scope 摘要相同时忽略仅 mtime 变化并返回 `Unchanged`，摘要变化时按 ID 稳定生成 Added/Modified/Deleted；仅根级未知字段变化会产生 records 为空的 `Changed`，不同 target 直接失败。错误事件只携带固定失败类别和可选逻辑记录 ID，不含路径、原始内容、凭据或自由格式错误文本；四领域共享合同但不共享文件读取、具体格式解析或归一化实现。TDD 红灯确认仅缺少生产合同；聚焦合同 5/5、串行 Rust 库 2390 项（5 ignored）、fmt/check、WSL 全目标 Clippy 和 diff-check 均通过；S.U.P.E.R 10/10。

- [x] **P5-02**: 实现启动、恢复、页面、5 秒/30 秒调度
  - Priority: P0
  - Effort: M
  - Dependencies: P5-01
  - Acceptance: 启动和主窗口从托盘恢复会扫描全部目标域，进入页面只扫描对应域；前台每 5 秒、托盘后台每 30 秒只比较摘要，所有周期任务可取消且不触发自动同步或网络访问。
  - Notes: 新增独立的摘要读取端口、固定 WSL 摘要适配器、串行扫描协调器和唯一后台调度 worker。应用启动立即扫描 Provider/MCP/Prompt/Skill × Claude/Codex/OpenCode 全部 12 个目标；进入页面只请求对应领域的三个客户端；关闭到托盘切换为 30 秒周期，托盘或单实例恢复切回 5 秒并立即全扫，退出发送取消命令。worker 串行等待每次 `spawn_blocking` 扫描结束，因此不会产生重叠扫描；扫描生产模块不依赖 WebDAV、HTTP、SQLite 或写入接口，只读取并散列固定受保护文件和 Skill 树。聚焦 Rust 合同 5/5 + 4/4、前端 Hook 1/1、前端全量 110 files/705 tests、串行 Rust 库 2390 passed/5 ignored、typecheck/format/fmt/check/全目标 Clippy/diff-check 均通过；Windows 原生测试进程直接读取真实 WSL UNC 且逐字节零修改 1/1；S.U.P.E.R 10/10。

- [x] **P5-03**: 实现变化后解析和自写抑制
  - Priority: P0
  - Effort: L
  - Dependencies: P5-01
  - Acceptance: 摘要不变时不解析完整内容；应用写入后的期望摘要与代次只抑制对应自写事件，后续第三方修改仍会被发现，错误路径失败关闭且不产生写入循环。
  - Notes: 新增独立 `LocalScanParserPort`、四领域固定完整解析适配器和仅驻留内存的 `LocalScanParsedChange`，摘要首次观察或未变化时不读取完整内容，只有摘要变化且非自写时才解析并更新待处理变化；解析失败保留最后成功摘要并在下次扫描重试，扫描层不依赖数据库、HTTP 或写入端口。新增进程级 `LocalScanWriteTracker`，业务写入完成后记录目标、期望摘要和单调写入代次；相同目标的匹配摘要只消费并抑制一次，不同摘要会清除过期期望并按外部变化解析，随后第三方修改不会被吞掉。Provider、MCP、Prompt、Skill 的所有生产 live 写入路径均在业务提交后登记实际写入目标；Codex/OpenCode 的 Provider 与 MCP 共用物理配置文件，登记时自动扩展另一领域并按目标去重，登记失败只输出固定脱敏类别且不反向破坏已提交操作。扫描/解析/协调合同 5/5 + 3/3 + 5/5 + 4/4，MCP/Prompt/Skill/通用片段直接依赖合同 5/5 + 4/4 + 5/5 + 4/4，前端全量 110 files/705 tests，串行 Rust 库 2390 passed/5 ignored，typecheck/format/fmt/check/全目标 Clippy/diff-check 均通过；Windows 原生测试进程在真实 WSL UNC 完成四领域完整解析、自写一次性抑制及后续第三方修改检测 1/1，隔离源码镜像和夹具已清理；S.U.P.E.R 10/10。

- [x] **P5-04**: 实现本地差异与冲突分类
  - Priority: P0
  - Effort: L
  - Dependencies: P5-03
  - Acceptance: 确定性新增、修改和删除形成可确认差异；不确定匹配、双边修改、无基线删除及解析错误形成冲突，任何记录在用户确认前均不得覆盖数据库或 live 配置。
  - Notes: 新增无基础设施依赖的三方分类核心，以最后确认基线、应用当前投影和 WSL live 解析状态生成稳定排序的 Added、Modified、Deleted 差异，并将无基线删除、不确定匹配、并发修改、更新/删除、解析失败和完整性异常独立归入冲突；单条冲突不会吞掉同批次可确认差异。解析后的 JSON 先递归规范化再计算 SHA-256，跨层合同只携带记录 ID、摘要和固定错误类别，不序列化原始配置或凭据。新增只读 `LocalReconciliationStatePort`，明确禁止以当前数据库投影冒充历史基线；协调器的 `classify_pending_from` 只预览、不消费待处理变化，也不具备数据库、live 文件或网络写入能力。成功解析状态与当前失败状态分开保留，解析失败会持续重试，临时读取失败只暂时遮蔽既有解析差异，恢复到相同摘要后原差异重新出现。Provider/MCP/Prompt/Skill × Claude/Codex/OpenCode 共 12 个目标合同覆盖；分类 6/6、扫描协调 7/7、相关聚焦 Rust 39/39、前端 110 文件/705 项、串行全量 Rust、fmt/check/全目标 Clippy、typecheck/format/diff-check 均通过；Windows 原生 MSVC 分类合同 6/6，通过纯领域边界证明不改变既有真实 WSL UNC 扫描与逐字节零写合同；临时源码镜像已清理。S.U.P.E.R 10/10。

- [x] **P5-05**: 实现统一冲突中心 UI 和处理流程
  - Priority: P0
  - Effort: L
  - Dependencies: P5-04、P2-03
  - Acceptance: 冲突中心统一展示供应商、MCP、Prompts、Skills 和后续 WebDAV 冲突；单条冲突不阻塞无冲突记录，处理前创建临时 DPAPI 回滚点，成功即删、失败最多保留 3 份。
  - Notes: 新增版本化、可序列化且不携带原始配置的统一冲突中心合同，以稳定来源、领域、客户端、记录 ID、摘要、固定差异/冲突类型和可执行动作同时承载本地扫描及后续 WebDAV 记录；多个来源经端口合并、稳定排序并拒绝重复 ID。处理前重新读取当前列表并拒绝陈旧项或不支持动作，再捕获领域回滚载荷并交给现有 Windows DPAPI 临时回滚存储；成功立即删除回滚点，失败保留且沿用最多 3 份的固定上限。生产处理适配器覆盖 Provider、MCP、Prompt、Skill 的接受 WSL 外部值、保留本地值、删除和重试，逐条应用、提交后校验并只消费目标项；单条冲突或数据库投影失败不会隐藏或阻塞其他可处理记录。前端新增持久化冲突中心视图、5 秒前台查询、按来源/领域/客户端筛选、摘要和错误类别展示、明确确认及相关查询失效，四语文案和工具栏入口同步完成。纯合同 5/5、生产运行时 7/7、前端聚焦 5 files/22 tests、App 8/8、前端全量 114 files/722 tests、串行 Rust 库 2390 passed/5 ignored 及全部 integration、typecheck/format/fmt/check/全目标 Clippy/diff-check 均通过；Windows 原生 MSVC 进程在真实 `\\wsl.localhost\Ubuntu` 临时根完成运行时合同 7/7，并显式断言 live 临时目录未退回 Windows 本地。质量门同时修复备份保留逻辑在系统时钟回拨时可能删除刚创建备份的问题，以未来 `mtime` 稳定回归 26/26；Windows 本地编译镜像、目标缓存和 WSL 夹具均已清理。S.U.P.E.R 10/10。

## Skill 本地索引刷新增量（2026-08-17）

本次重新打开 Phase 5，用于修正 Skill managed 索引长期只由数据库驱动、无法在显式导入入口反映磁盘删除和元数据变化的问题。已确认语义如下：

- 启动、恢复、进入页面、前台 5 秒和托盘后台 30 秒扫描仍然只读，只生成差异/冲突，不自动修改数据库或 WSL。
- background Skill scan 只扫描 `core_skills` 已知目录，unknown 目录只在用户点击“导入已有”后扫描；fresh process 首轮以 `core_skills` 已确认内容哈希和 `apps` 为基线，检测停机期间的修改、删除和已知目录新增，未确认哈希强制解析。
- 点击“导入已有”是显式本地索引刷新授权：先安全列出 Claude Code、Codex、OpenCode 三个固定 WSL Skill 根，刷新 managed 列表，再展示 unmanaged 候选；该动作不写、不复制、不覆盖 WSL。
- 某端目录确认删除时关闭该端，三端均确认删除时移出 managed 索引；安全有效且内容一致的现存副本刷新 `name`、`description`、内容哈希、大小、文件数、云同步资格和 `apps`。
- 多端副本内容分叉时保留 canonical 元数据，只按安全确认存在的副本刷新 `apps` 并显示冲突；目录重命名按“旧 managed 删除 + 新 unmanaged”处理，不猜测身份。
- WSL home/固定根不可用必须在数据库变更前失败关闭；link/reparse、大小写别名、重复候选和 invalid copy 不得造成误删。无问题记录可继续刷新，但异常记录保持原 managed 状态。
- 索引 upsert/delete 必须在单个 SQLite transaction 中提交；只有确有索引修改时才创建最多 3 个 DPAPI 加密临时回滚点，无变化纯扫描不创建。
- 设计只借鉴 SkillManage“本地磁盘驱动索引刷新”的理念；仍固定三端和既定 WSL 根，不引入任意 Agent/路径、`.disabled` 或自动覆盖。

最终工作树已完成 DB-known background scan、fresh-process confirmed hash 基线、显式 managed/unmanaged 扫描、删除/一致刷新/分叉/重命名/异常保护、SQLite transaction 和 DPAPI 临时回滚点接入。完整前端 43 文件/229 项、Rust 29 目标/381 项、严格 Clippy、格式、diff-check、真实 WSL UNC Skill 合同 1/1、Windows x64 便携构建、校验和及隔离启动烟测均通过；独立复核另修正了“数据库已提交但回滚点清理失败时不得向前端伪报扫描失败”的一致性边缘，并新增回归测试。

## Phase Gate

- [x] 原 Phase 5 五项任务的既有基线已完成。
- [x] 本次 Skill 本地索引刷新增量已完成自动化、Windows/WSL 隔离合同、回滚路径和便携构建验证，阶段门重新关闭。
- [x] 四个本地领域共享扫描契约，但解析和归一化保持独立。
- [x] 启动、恢复、页面、前台 5 秒和后台 30 秒触发行为均可取消并通过测试。
- [x] 无变化扫描不做完整解析，扫描本身不写数据库或 WSL live 配置。
- [x] 应用自写不循环，写后第三方修改不会被抑制。
- [x] 外部新增、修改、删除、双边修改、无基线删除和解析错误分类正确。
- [x] 冲突记录保持原状，无冲突记录可以继续处理。
- [x] 路径越界、link/reparse、循环和权限错误失败关闭并只产生脱敏诊断。
- [x] Frontend、Rust、Windows 原生到 WSL UNC 原阶段门全部通过。
- [x] S.U.P.E.R review is 10/10 for every originally completed task.
- [x] background Skill scan 只读取 DB-known 目录，fresh process 使用 confirmed hash，unknown 仅由“导入已有”枚举。
- [x] 显式刷新已覆盖单端/全端删除、一致副本、内容分叉、重命名、home 不可用、link/reparse、大小写别名和 invalid copy 矩阵。
- [x] 索引有修改才创建、无变化不创建最多 3 个 DPAPI 临时回滚点；SQLite 批次全有或全无且刷新过程 WSL 零写。
- [x] 完整前端、Rust、Windows/WSL 隔离合同和便携构建质量门通过；最终产物元数据已按实际文件更新。

## Notes

- 扫描只读取固定 WSL Ubuntu 路径；不得恢复可编辑目录、任意发行版或任意用户选择器。
- 本地摘要轮询不得访问 WebDAV，也不得自动把数据库状态写入 live 配置或把外部状态导入数据库。
- 共享契约只描述摘要、来源、变化和错误，不得把 Claude/Codex/OpenCode 具体文件格式耦合进统一核心。
- WebDAV 三方合并与云冲突在 Phase 6 实现，本阶段只预留统一冲突来源，不提前建立旧整包同步依赖。
