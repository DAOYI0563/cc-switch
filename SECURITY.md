# 安全策略

仅最新发布版本接收安全修复。

## 威胁模型

WSL Code Switch 是本地 Windows 桌面应用，没有项目运营的云后端。它以当前 Windows 用户权限读取和更新固定 WSL Ubuntu 用户目录中的 Claude Code、Codex 和 OpenCode 配置。

需要视为不可信输入的边界包括：

- WSL live 配置和本地会话文件；
- WebDAV 返回的密文、manifest、ETag 和 HTTP 状态；
- 三个 CLI 与官方版本源的输出；
- 每日简报 AI 接口返回的模型文本；
- 用户填写的供应商、MCP、Prompt、Skill 和同步配置。

应用不包含本地 HTTP 代理、Deep Link、S3、自动同步、远程可执行前端内容或应用内更新器。

## 安全属性

- WSL 路径固定到 `Ubuntu`/`zhldm`，拒绝越界、symlink 和 reparse-point 逃逸。
- live 配置采用同目录临时文件与原子替换；失败不得以空内容覆盖原文件。
- 高风险本地应用操作使用 DPAPI 加密临时回滚点，最多保留 3 个。
- WebDAV 密码进入 Windows Credential Manager；同步口令只在当次操作内存中使用。
- sync-v3 远端对象使用 Argon2id 与 AES-256-GCM，逐对象唯一 nonce，并以 ETag 条件写防止静默覆盖。
- 原始会话、搜索索引、恢复命令、AI Key、检查点和设备级设置禁止进入同步载荷。
- 每日简报 HTML 不含脚本和外链；模型输出必须转义，只有完整且哈希校验通过的结果可同步。
- 日志和诊断不得记录 API Key、认证头、Cookie、同步口令或 WebDAV 密码。

## 报告漏洞

不要在公开 Issue 中披露漏洞。请通过仓库的 GitHub Security Advisory 私下报告，并提供：受影响版本、可复现步骤、不可信输入来源、完整数据路径和影响。

仅通过 DevTools 或本地修改前端直接调用 IPC、且没有任何不可信输入参与的问题，不构成跨信任边界漏洞。
