# Phase 0 Contract Fixtures

These fixtures freeze the input shapes that the WSL Code Switch refactor must
continue to understand. They are intentionally small, reviewable, and contain
only fictional projects, example domains, environment-variable references, and
placeholder credentials.

## Layout

- `v16/cc-switch-v16.sql`: representative schema-v16 data containing both
  retained records and legacy records that later migrations must ignore or
  remove safely.
- `v16/settings.json`: representative device settings with allowed local
  preferences, a legacy WebDAV password, and fields that must be discarded.
- `live/claude/settings.json`: Claude Code live settings with unknown fields.
- `live/codex/config.toml`: Codex live configuration with comments, profiles,
  MCP configuration, and unknown fields.
- `live/codex/auth.json`: fictional official-login shape that custom provider
  writes must preserve byte-for-byte.
- `live/opencode/opencode.jsonc`: OpenCode JSONC configuration with unknown
  fields at the document, provider, option, model, and MCP levels.
- `sessions/claude/session.jsonl`: representative Claude Code conversation.
- `sessions/codex/session.jsonl`: representative Codex conversation.
- `sessions/opencode/storage/`: representative OpenCode JSON storage layout.
- `sessions/opencode/opencode-sessions.sql`: representative OpenCode SQLite
  session data, expressed as portable SQL rather than a binary database.

Do not replace placeholders with data copied from a real user profile.
