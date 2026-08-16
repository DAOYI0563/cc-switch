# WSL Code Switch 项目约定

## 产品边界

- 仅支持 Windows 11 x64、WSL2 `Ubuntu`、WSL 用户 `zhldm` 和 Windows Terminal。
- 仅管理 Claude Code、Codex、OpenCode。
- 仅发布 Windows x64 便携 EXE，不恢复安装器或其他平台桌面包。
- WebDAV 只能由用户手动触发，使用 sync-v3 逐记录双向 E2EE；禁止自动同步、后台重试和 S3。
- 会话数据只读且永不上传；只有完整并通过哈希校验的每日简报可进入加密同步。
- 禁止恢复代理、路由接管、OAuth、故障转移、用量统计、Profiles、Deep Link、Updater、OMO 和非目标客户端。

## 开发规则

- 修改代码前先搜索调用链和测试，不猜文件位置。
- WSL live 配置写入必须经过固定路径解析、安全边界检查和同目录原子替换。
- 高风险本地应用操作必须使用最多 3 个 DPAPI 加密临时回滚点。
- 新增用户可见文本只维护 `src/i18n/locales/zh.json`。
- 手工编辑使用 `apply_patch`，不得覆盖无关的工作区改动。

## 验证命令

```bash
pnpm typecheck
pnpm format:check
pnpm test:unit
cargo fmt --check --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml -- --test-threads=1
```

Windows 原生构建：

```powershell
pnpm run build:portable
```

权威产品规格见 `docs/plan/wsl-code-switch-refactor-plan.md`，当前交付状态见 `docs/progress/MASTER.md`。
