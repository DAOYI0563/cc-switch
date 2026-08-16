# Phase 1: Windows/WSL 平台底座和品牌

**Goal**: 建立唯一、可测试的 Windows 11 + WSL2 Ubuntu 平台事实来源，并完成产品及交付目标收敛。  
**Status**: Complete

## Tasks

- [x] **P1-01**: 实现固定 `WslPathResolver`
  - Priority: P0
  - Effort: M
  - Dependencies: P0-04
  - Acceptance: Claude Code、Codex、OpenCode 的固定 Windows UNC 根目录、WSL 根目录和明确例外路径均由单一适配器提供；领域层不依赖 Windows 路径。
  - Notes: `ports/wsl_paths` 定义纯契约，`adapters/wsl_paths` 固定 `Ubuntu`/`zhldm`；三端配置、Claude 状态文件和 OpenCode 会话根均从单一适配器解析。目标客户端的旧目录覆盖 getter 已停止活动使用。

- [x] **P1-02**: 实现 UNC 安全解析、越界和 symlink 防护
  - Priority: P0
  - Effort: L
  - Dependencies: P1-01
  - Acceptance: 路径规范化后仅允许位于所属固定根目录；越界、循环、symlink/reparse-point 逃逸、非法客户端和不可解析路径均失败关闭。
  - Notes: `SafeWslPathGuard` 按 scope 和读写意图解析相对路径，拒绝绝对路径、父目录、分隔符伪装、symlink/reparse point、循环和只读会话写入；Linux/WSL 合同 5/5，Windows 原生 WSL UNC symlink 逃逸合同 1/1。

- [x] **P1-03**: 实现 WSL UNC 原子读写适配器
  - Priority: P0
  - Effort: M
  - Dependencies: P1-01
  - Acceptance: 写入使用同目录临时文件、同步和替换；失败保持原文件；Windows 原生进程对 WSL2 UNC 的契约测试通过。
  - Notes: `WslFileSystem` 端口和 `WslFileAdapter` 实现受控读取、同目录临时文件与原子替换，失败保持原文件；本地合同 4/4，Windows 原生 WSL UNC 合同 1/1。

- [x] **P1-04**: 更名产品、identifier 和数据目录
  - Priority: P0
  - Effort: M
  - Dependencies: P1-01
  - Acceptance: 产品名称为 `WSL Code Switch`，identifier 为 `com.zhldm.wsl-code-switch`，Windows 数据根目录为 `%USERPROFILE%\.wsl-code-switch`；迁移前不删除 `.cc-switch`。
  - Notes: 产品、NPM/Rust 包、Rust lib、Tauri identifier、窗口/HTML 标题和日志前缀已更名；应用数据根固定为 `%USERPROFILE%\.wsl-code-switch`，覆盖 IPC、Store 缓存、设置 UI 和重启链已删除；旧 `.cc-switch` 不删除，仅留作 Phase 2 只读迁移来源。前端定向 33/33、Rust 固定目录 1/1、panic hook 3/3、`cargo check` 和 clippy `-D warnings` 通过。

- [x] **P1-05**: 将构建目标限制为 Windows x64 便携式 EXE
  - Priority: P1
  - Effort: M
  - Dependencies: P1-04
  - Acceptance: Tauri/CI/release 配置仅生成 Windows x64 便携式 EXE，不再声明 macOS、Linux、ARM、安装包或 updater 产物。
  - Notes: 2026-08-14 用户将原 MSI-only 决策改为 portable EXE-only；Tauri bundle 已关闭，WiX 模板已删除，release 仅复制 `x86_64-pc-windows-msvc/release/wsl-code-switch.exe`。此前 Windows 原生 MSI 验证仅作为历史记录，不再作为最终交付证据；最终 EXE 实机证据在 P11-02/P11-03 补齐。

## Phase Notes

- 固定 WSL 发行版为 `Ubuntu`，用户为 `zhldm`，终端为 Windows Terminal。
- 配置写入仅允许三个配置根；`.claude.json` 是 Claude 初次确认状态的单文件例外；OpenCode `.local/share/opencode` 仅可只读扫描会话。
- 测试覆盖必须包含 Windows 风格 UNC 字符串的纯单元测试和宿主 Windows 实机合同。

## Phase Completion Checklist

- [x] All tasks above are checked off.
- [x] All configuration paths are resolved by one adapter.
- [x] Path traversal and link escape contracts fail closed.
- [x] Native Windows-to-WSL atomic read/write contracts pass.
- [x] Product metadata and data-root tests pass.
- [x] Only the Windows x64 portable EXE remains as a delivery target.
- [x] `MASTER.md` phase count and active phase are updated.
