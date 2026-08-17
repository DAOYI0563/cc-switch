# WSL Code Switch 交付状态

> 最后更新：2026-08-17<br>
> 状态：Phase 5 Skill 本地索引刷新增量已完成验证；原交付基线保留<br>
> 产品范围：Windows 11 x64 + WSL2 `Ubuntu` + 用户 `zhldm` + Windows Terminal

## 总体进度

|    Phase | 内容                 |                                    完成 |
| -------: | -------------------- | --------------------------------------: |
|        0 | 基线与领域契约       |                                     4/4 |
|        1 | Windows/WSL 平台底座 |                                     5/5 |
|        2 | 数据迁移与凭据       |                                     6/6 |
|        3 | 三端供应商核心       |                                     8/8 |
|        4 | MCP、Prompt、Skill   |                                     5/5 |
|        5 | 本地扫描与冲突中心   |                   5/5；Skill 增量已验证 |
|        6 | 手动 WebDAV sync-v3  |                                     9/9 |
|        7 | 只读会话与 CLI 状态  |                                     6/6 |
|        8 | 每日 AI 工作简报     |                                   10/10 |
|        9 | 界面、托盘与诊断     |                                     5/5 |
|       10 | 旧功能物理清理       |                                     7/7 |
|       11 | Windows 交付与验收   |                                     4/5 |
| **总计** |                      |      **74/75 原基线；Skill 增量已验证** |

Phase 11 唯一未关闭项是使用真实 WebDAV 服务和两台实体 Windows/WSL 设备执行全流程环境验收。双设备合并、CAS、冲突、墓碑、E2EE 和零写失败路径已有自动化双运行时合同覆盖，但该合同不替代实体设备结论。

2026-08-17 重新打开的 Phase 5 Skill 本地索引刷新增量不改变上述原交付基线计数。该增量现已通过完整前端、Rust、真实 WSL UNC 隔离合同、便携构建、校验和与启动烟测，以下数值均为最终工作树的重新验证证据。

## 最终产物

- 文件：`dist-portable/WSL-Code-Switch-3.19.4-Windows-x64-portable.exe`
- 大小：`18,505,216` 字节
- SHA-256：`55a523280db3144f0e770983ee3a891addae727d5d3b23153c0b3926891be73e`
- 架构：PE32+ GUI x86-64
- 签名：未签名
- 构建命令：`pnpm run build:portable`

交付目录只包含上述 EXE 与同名 `.sha256` 文件。

## 验证证据

- 前端：`pnpm format:check`、`pnpm typecheck` 通过；43 个测试文件、229 项测试全部通过，其中 Skill 管理页 23 项覆盖即时删除、元数据/客户端刷新、异常标记、失败保留旧列表和显式来源导入。
- Rust：`cargo fmt --check` 通过；全目标 Clippy `-D warnings` 通过；默认全量 29 个测试目标共 381 项通过、7 项环境测试忽略，所有 integration tests 通过。
- Windows/WSL：原生 Windows Rust 进程在隔离 `//wsl.localhost/Ubuntu/tmp/cc-switch-final-skill-test` 夹具完成 Skill 复制、同步、冲突阻断和符号链接拒绝 1/1，夹具已清理。
- Windows 构建：`x86_64-pc-windows-msvc` release 构建成功；最终 EXE 版本 `3.19.4`、PE machine `0x8664`、未签名，同名 SHA-256 文件校验通过。
- 启动烟测：使用独立 `CC_SWITCH_TEST_HOME` 启动最终便携 EXE，进程运行 10 秒未提前退出；测试进程和临时目录已清理。当前 GUI 控制器仅支持内置网页标签，不能接管独立 Tauri/WebView2 窗口，因此未将最终原生 GUI 黑盒截图列为通过证据。
- 打包合同：无安装器、Updater、Deep Link 或非 Windows 发布目标；最终目录为 EXE + SHA-256。

## 已实现范围

- Claude Code、Codex、OpenCode 供应商、MCP、Prompt 和本地 Skill 管理。
- WSL live 配置扫描、统一冲突中心、原子写入与最多 3 个 DPAPI 临时回滚点。
- 用户手动触发的 sync-v3：首次预览、设备登记/退役、逐记录双向合并、永久墓碑、ETag CAS、Argon2id + AES-256-GCM。
- 三端只读会话浏览、搜索、Windows Terminal 恢复；CLI 版本状态与仅复制升级命令。
- 每日离线 HTML 简报、脱敏/限额/完整性边界，以及仅完整简报的加密同步。
- 简体中文界面、精简托盘、Windows x64 便携发布。

本次增量的已确认语义是：所有启动、恢复、页面进入和 5 秒/30 秒 Skill scan 保持只读，仅扫描数据库已知目录并生成差异/冲突；fresh process 首轮以 `core_skills` 已确认哈希检测停机期间变化，unknown 目录不进入 background scan。

用户点击 Skill“导入已有”被定义为显式本地索引刷新授权：先安全扫描 Claude Code、Codex、OpenCode 三个固定 WSL Skill 根并刷新 managed 列表，再展示 unmanaged 候选。删除、一致副本、内容分叉、目录重命名和异常副本的处理遵循权威规格第 6.4/7.3 节；整个刷新不写或覆盖 WSL。该设计只取 SkillManage“本地磁盘驱动索引刷新”的理念，不引入任意 Agent/路径、`.disabled` 或自动覆盖。本次增量的自动化、真实 WSL UNC 隔离合同和便携构建质量门均已关闭。

## 下一工作点

1. Phase 11 仍需真实 WebDAV 服务和两台实体 Windows/WSL 设备全流程环境验收。
2. 若后续环境提供可接管独立 Tauri/WebView2 窗口的 GUI 自动化能力，再补充最终便携 EXE 的截图级黑盒回归；该环境缺口不替代现有组件、原生 WSL UNC 与启动烟测结论。

## 明确排除

代理、路由接管、协议转换、OAuth、故障转移、用量统计、Profiles、Deep Link、Updater、S3、OMO、Gemini、Grok Build、OpenClaw、Hermes、Claude Desktop、macOS/Linux/移动端桌面包和安装器均不属于产品。

## 权威资料

- [原始重构规格](../plan/wsl-code-switch-refactor-plan.md)
- [后续开发与验收计划](../plan/wsl-code-switch-follow-up-development-plan.md)
- [简体中文用户手册](../user-manual/README.md)
