# 贡献指南

提交改动前请先阅读 [项目约定](AGENTS.md) 和 [用户手册](docs/user-manual/README.md)。功能提案不得突破固定产品边界。

## 环境

- Windows 11 x64
- WSL2 `Ubuntu`，用户 `zhldm`
- Node.js 22、Corepack、pnpm 10.12.3
- Rust stable 与 `x86_64-pc-windows-msvc` target
- Tauri 2 的 Windows 构建依赖

## 开发

```bash
corepack enable
pnpm install --frozen-lockfile
pnpm dev
```

用户可见文本固定为简体中文，只修改 `src/i18n/locales/zh.json`。前端使用严格 TypeScript 和 Prettier，Rust 必须通过 `rustfmt` 与零警告 Clippy。

## 提交前验证

```bash
pnpm typecheck
pnpm format:check
pnpm test:unit
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

涉及 WSL UNC、Credential Manager、DPAPI、Windows Terminal 或发布产物时，还必须运行 Windows 原生合同和便携版构建：

```powershell
pnpm run build:portable
```

每个 PR 只解决一个问题，并说明测试证据。不得提交真实 API Key、WebDAV 凭据、用户会话或真实用户路径。
