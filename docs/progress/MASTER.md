# WSL Code Switch 交付状态

> 最后更新：2026-08-16  
> 状态：软件实现完成，Skill 导入冲突误报已修复，最终便携版已生成  
> 产品范围：Windows 11 x64 + WSL2 `Ubuntu` + 用户 `zhldm` + Windows Terminal

## 总体进度

| Phase | 内容 | 完成 |
| ---: | --- | ---: |
| 0 | 基线与领域契约 | 4/4 |
| 1 | Windows/WSL 平台底座 | 5/5 |
| 2 | 数据迁移与凭据 | 6/6 |
| 3 | 三端供应商核心 | 8/8 |
| 4 | MCP、Prompt、Skill | 5/5 |
| 5 | 本地扫描与冲突中心 | 5/5 |
| 6 | 手动 WebDAV sync-v3 | 9/9 |
| 7 | 只读会话与 CLI 状态 | 6/6 |
| 8 | 每日 AI 工作简报 | 10/10 |
| 9 | 界面、托盘与诊断 | 5/5 |
| 10 | 旧功能物理清理 | 7/7 |
| 11 | Windows 交付与验收 | 4/5 |
| **总计** |  | **74/75（98.7%）** |

Phase 11 唯一未关闭项是使用真实 WebDAV 服务和两台实体 Windows/WSL 设备执行全流程环境验收。双设备合并、CAS、冲突、墓碑、E2EE 和零写失败路径已有自动化双运行时合同覆盖，但该合同不替代实体设备结论。

## 最终产物

- 文件：`dist-portable/WSL-Code-Switch-3.19.4-Windows-x64-portable.exe`
- 大小：`18,304,000` 字节
- SHA-256：`e826635901fb362a08869bed7782207e85a95beb30b508d9176e8197cf8bb0d8`
- 架构：PE32+ GUI x86-64
- 签名：未签名
- 构建命令：`pnpm run build:portable`

交付目录只包含上述 EXE 与同名 `.sha256` 文件。

## 验证证据

- 前端：`pnpm format:check`、`pnpm typecheck` 通过；43 个测试文件、218 项测试全部通过。
- Rust：`cargo fmt --check` 通过；全目标 Clippy `-D warnings` 通过；library 201 项及全部 integration tests 通过。
- Windows 构建：`x86_64-pc-windows-msvc` release 构建成功。
- 启动烟测：使用独立 `CC_SWITCH_TEST_HOME` 启动 `3.19.4`，通过 WebView2 DOM 进入 Skill 管理并打开“导入已有”；真实三端 WSL 目录扫描期间进度可见，识别 44 个可导入 Skill，逐行审计确认 44/44 均只默认选中明确来源端，进程保持响应，未执行最终导入；测试进程和临时目录已清理。
- 打包合同：无安装器、Updater、Deep Link 或非 Windows 发布目标；最终目录为 EXE + SHA-256。

## 已实现范围

- Claude Code、Codex、OpenCode 供应商、MCP、Prompt 和本地 Skill 管理。
- WSL live 配置扫描、统一冲突中心、原子写入与最多 3 个 DPAPI 临时回滚点。
- 用户手动触发的 sync-v3：首次预览、设备登记/退役、逐记录双向合并、永久墓碑、ETag CAS、Argon2id + AES-256-GCM。
- 三端只读会话浏览、搜索、Windows Terminal 恢复；CLI 版本状态与仅复制升级命令。
- 每日离线 HTML 简报、脱敏/限额/完整性边界，以及仅完整简报的加密同步。
- 简体中文界面、精简托盘、Windows x64 便携发布。

## 明确排除

代理、路由接管、协议转换、OAuth、故障转移、用量统计、Profiles、Deep Link、Updater、S3、OMO、Gemini、Grok Build、OpenClaw、Hermes、Claude Desktop、macOS/Linux/移动端桌面包和安装器均不属于产品。

## 权威资料

- [原始重构规格](../plan/wsl-code-switch-refactor-plan.md)
- [后续开发与验收计划](../plan/wsl-code-switch-follow-up-development-plan.md)
- [简体中文用户手册](../user-manual/README.md)
