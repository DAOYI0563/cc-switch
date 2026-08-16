# Phase 0: 冻结基线和契约

**Goal**: 在大规模删除前建立可复现的测试基线、代表性夹具和稳定领域契约。  
**Status**: Complete

## Tasks

- [x] **P0-01**: 建立 v16 数据库、三端配置和会话测试夹具
  - Priority: P0
  - Effort: M
  - Dependencies: None
  - Acceptance: 夹具覆盖目标保留数据、主要旧数据形态、Claude/Codex/OpenCode live 配置和会话来源；不包含真实凭据或私人会话。
  - Notes: 已建立脱敏 v16 SQL、Claude/Codex/OpenCode live 配置，以及 Claude/Codex JSONL、OpenCode JSON storage/SQLite 会话夹具；专项合同测试通过。

- [x] **P0-02**: 固化三端 live 配置往返测试
  - Priority: P0
  - Effort: M
  - Dependencies: P0-01
  - Acceptance: 三端配置 parse/write/parse 保留未知字段、官方登录材料和客户端专属结构；失败写入不破坏原文件。
  - Notes: 三端往返合同覆盖 Claude JSON 未知字段、Codex TOML 注释/未知字段/官方 `auth.json`、OpenCode JSONC 未知语义和失败零写入；`8/8` 通过。

- [x] **P0-03**: 记录当前前后端测试基线
  - Priority: P1
  - Effort: S
  - Dependencies: None
  - Acceptance: 记录 frontend typecheck/format/tests、Rust fmt/clippy/tests 和 Windows+WSL2 契约的真实结果，区分环境缺失与源码失败。
  - Notes: 前端 typecheck、format、108 个测试文件/738 项测试通过；Rust fmt、clippy、库测试 2358 通过/5 项环境忽略及全部集成测试通过；Windows 原生进程对 WSL UNC 原子写入合同 `1/1` 通过。

- [x] **P0-04**: 定义领域 ID、序列化 schema 和错误类型
  - Priority: P0
  - Effort: M
  - Dependencies: P0-01
  - Acceptance: 三端应用 ID、可移植记录 ID、同步记录、冲突、会话事件和每日简报状态具备显式可序列化契约；领域类型不依赖 Tauri/SQLite/Windows。
  - Notes: 已定义三个托管客户端、可移植领域/记录/版本/墓碑/冲突、归一化会话事件、每日简报状态和可序列化领域错误；纯领域合同 `5/5` 通过且不依赖 Tauri、SQLite 或 Windows。

## Phase Notes

- Phase 0 只冻结目标行为，不删除旧功能。
- Windows/WSL 原生契约是阶段门的一部分；WSL 内部测试不能替代宿主 Windows 证据。
- 公共契约冻结后，Phase 1 和后续适配器才允许并行开发。

## Phase Completion Checklist

- [x] All tasks above are checked off.
- [x] Frontend and Rust baseline results are persisted in `MASTER.md`.
- [x] Representative fixtures contain no real credentials or private session content.
- [x] Three-client roundtrip contracts pass.
- [x] Domain contracts pass the S.U.P.E.R checklist.
- [x] `MASTER.md` phase count and active phase are updated.

## Acceptance Evidence

| Check | Result | Command / evidence |
|---|---|---|
| Frontend types | Pass | `pnpm typecheck` |
| Frontend format | Pass | `pnpm format:check` |
| Frontend unit tests | Pass | `pnpm test:unit`: 108 files, 738 tests |
| Rust format | Pass | `cargo fmt --check --manifest-path src-tauri/Cargo.toml` |
| Rust lint | Pass | `cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings` |
| Rust full suite | Pass | `cargo test --manifest-path src-tauri/Cargo.toml`: library 2358 passed, 5 environment tests ignored; all integration tests passed |
| Fixture contracts | Pass | `cargo test --manifest-path src-tauri/Cargo.toml phase0_fixture_contracts --lib`: 8/8 |
| Domain contracts | Pass | `cargo test --manifest-path src-tauri/Cargo.toml domain::contracts --lib`: 5/5 |
| Native Windows -> WSL UNC | Pass | Native `cargo.exe` test process with `CC_SWITCH_TEST_HOME`, `CC_SWITCH_WSL_TEST_DIR`, `TEMP`, and `TMP` under `\\wsl.localhost\Ubuntu\tmp\wsl-code-switch-native-test`: 1/1 |
| Patch hygiene | Pass | `git diff --check` |

Windows test compilation used a Windows-local source mirror because MSVC `mt.exe` cannot link a manifest from a UNC source tree. The executed test itself used WSL UNC paths for its test root, home, temporary directory, and atomic-write destination, so the required host-to-WSL filesystem behavior was exercised by a native Windows binary.
