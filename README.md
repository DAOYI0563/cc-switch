# WSL Code Switch

WSL Code Switch 是面向固定 Windows/WSL 环境的本地配置管理工具。它只管理 WSL Ubuntu 用户 `zhldm` 下的 Claude Code、Codex 和 OpenCode，并只提供 Windows x64 便携版。

## 环境要求

- Windows 11 x64
- WSL2，发行版名称为 `Ubuntu`
- WSL 用户名为 `zhldm`
- Windows Terminal
- 已安装至少一个目标 CLI

## 功能

- 管理、导入和切换 Claude Code、Codex、OpenCode 供应商配置
- 管理三端 MCP、Prompt 和本地 Skill
- 扫描 WSL live 配置，在冲突中心显式确认差异
- 只读浏览、搜索和恢复本机会话
- 查询 CLI 版本并生成可复制的升级命令，不自动升级
- 生成离线 HTML 每日工作简报
- 通过 WebDAV 手动执行逐记录、端到端加密的双向同步
- 通过系统托盘快速打开应用或切换供应商

应用不包含代理、路由接管、OAuth、故障转移、用量统计、Profiles、Deep Link、自动更新或其他客户端。

## 使用

下载 `WSL-Code-Switch-<version>-Windows-x64-portable.exe` 后直接运行，无需安装。首次启动会读取旧版本数据，迁移保留内容，并忽略已删除功能的数据。

详细操作见 [用户手册](docs/user-manual/README.md)。

## 本地构建

```powershell
corepack enable
pnpm install --frozen-lockfile
pnpm run build:portable
```

产物位于：

```text
src-tauri/target/x86_64-pc-windows-msvc/release/wsl-code-switch.exe
```

## 验证

```bash
pnpm typecheck
pnpm format:check
pnpm test:unit
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

## 数据安全

- WebDAV 密码保存到 Windows Credential Manager。
- 同步口令只在本次操作的内存中使用。
- 原始会话、会话索引、AI Key 和简报检查点不会上传。
- WebDAV 远端对象使用 Argon2id 与 AES-256-GCM 加密。
- 配置写入采用原子替换；高风险写入使用最多 3 个加密临时回滚点。

许可证：[MIT](LICENSE)
