PRAGMA foreign_keys = OFF;

CREATE TABLE providers (
    id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    name TEXT NOT NULL,
    settings_config TEXT NOT NULL,
    website_url TEXT,
    category TEXT,
    created_at INTEGER,
    sort_index INTEGER,
    notes TEXT,
    icon TEXT,
    icon_color TEXT,
    meta TEXT NOT NULL DEFAULT '{}',
    is_current BOOLEAN NOT NULL DEFAULT 0,
    in_failover_queue BOOLEAN NOT NULL DEFAULT 0,
    PRIMARY KEY (id, app_type)
);

CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    server_config TEXT NOT NULL,
    description TEXT,
    homepage TEXT,
    docs TEXT,
    tags TEXT NOT NULL DEFAULT '[]',
    enabled_claude BOOLEAN NOT NULL DEFAULT 0,
    enabled_codex BOOLEAN NOT NULL DEFAULT 0,
    enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
    enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
    enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
    enabled_hermes BOOLEAN NOT NULL DEFAULT 0
);

CREATE TABLE prompts (
    id TEXT NOT NULL,
    app_type TEXT NOT NULL,
    name TEXT NOT NULL,
    content TEXT NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT 1,
    created_at INTEGER,
    updated_at INTEGER,
    PRIMARY KEY (id, app_type)
);

CREATE TABLE skills (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    directory TEXT NOT NULL,
    repo_owner TEXT,
    repo_name TEXT,
    repo_branch TEXT DEFAULT 'main',
    readme_url TEXT,
    enabled_claude BOOLEAN NOT NULL DEFAULT 0,
    enabled_codex BOOLEAN NOT NULL DEFAULT 0,
    enabled_gemini BOOLEAN NOT NULL DEFAULT 0,
    enabled_grokbuild BOOLEAN NOT NULL DEFAULT 0,
    enabled_opencode BOOLEAN NOT NULL DEFAULT 0,
    enabled_hermes BOOLEAN NOT NULL DEFAULT 0,
    installed_at INTEGER NOT NULL DEFAULT 0,
    content_hash TEXT,
    updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);

CREATE TABLE profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    payload TEXT NOT NULL,
    sort_order INTEGER,
    created_at INTEGER,
    updated_at INTEGER
);

CREATE TABLE proxy_request_logs (
    id INTEGER PRIMARY KEY,
    app_type TEXT,
    provider_id TEXT,
    model TEXT,
    created_at TEXT
);

CREATE TABLE usage_daily_rollups (
    date TEXT NOT NULL,
    app_type TEXT NOT NULL,
    provider_id TEXT NOT NULL,
    request_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (date, app_type, provider_id)
);

INSERT INTO providers (
    id, app_type, name, settings_config, category, created_at, sort_index,
    meta, is_current, in_failover_queue
) VALUES
    ('fixture-claude', 'claude', 'Fixture Claude',
     '{"env":{"ANTHROPIC_BASE_URL":"https://claude.example.invalid","ANTHROPIC_AUTH_TOKEN":"{env:FIXTURE_CLAUDE_TOKEN}"}}',
     'custom', 1700000000000, 0, '{}', 1, 0),
    ('fixture-codex', 'codex', 'Fixture Codex',
     '{"auth":{"OPENAI_API_KEY":"{env:FIXTURE_CODEX_TOKEN}"},"config":"model = \"fixture-model\"\n"}',
     'custom', 1700000001000, 0, '{}', 1, 0),
    ('fixture-opencode', 'opencode', 'Fixture OpenCode',
     '{"npm":"@ai-sdk/openai-compatible","options":{"baseURL":"https://opencode.example.invalid/v1","apiKey":"{env:FIXTURE_OPENCODE_TOKEN}"},"models":{}}',
     'custom', 1700000002000, 0, '{}', 1, 0),
    ('legacy-gemini', 'gemini', 'Legacy Gemini', '{}', 'official',
     1700000003000, 0, '{}', 0, 0);

INSERT INTO mcp_servers (
    id, name, server_config, tags, enabled_claude, enabled_codex,
    enabled_gemini, enabled_grokbuild, enabled_opencode, enabled_hermes
) VALUES (
    'fixture-mcp', 'Fixture MCP',
    '{"command":"fixture-mcp","args":["--stdio"]}', '[]', 1, 1, 0, 0, 1, 0
);

INSERT INTO prompts (
    id, app_type, name, content, enabled, created_at, updated_at
) VALUES
    ('fixture-prompt-claude', 'claude', 'Fixture CLAUDE.md',
     '# Fixture instructions', 1, 1700000000000, 1700000000000),
    ('fixture-prompt-codex', 'codex', 'Fixture AGENTS.md',
     '# Fixture instructions', 1, 1700000000000, 1700000000000),
    ('legacy-prompt-gemini', 'gemini', 'Legacy prompt',
     '# Legacy fixture', 1, 1700000000000, 1700000000000);

INSERT INTO skills (
    id, name, directory, enabled_claude, enabled_codex, enabled_gemini,
    enabled_grokbuild, enabled_opencode, enabled_hermes, installed_at, updated_at
) VALUES (
    'fixture-skill', 'Fixture Skill', 'fixture-skill', 1, 1, 0, 0, 1, 0,
    1700000000000, 1700000000000
);

INSERT INTO settings (key, value) VALUES
    ('current_provider_claude', 'fixture-claude'),
    ('current_provider_codex', 'fixture-codex'),
    ('common_config_claude', '{"permissions":{"allow":["Read"]}}'),
    ('common_config_codex', 'model_reasoning_effort = "high"'),
    ('legacy_auto_sync', 'true');

INSERT INTO profiles (id, name, payload, sort_order, created_at, updated_at)
VALUES ('legacy-profile', 'Legacy Profile', '{}', 0, 1700000000000, 1700000000000);

INSERT INTO proxy_request_logs (id, app_type, provider_id, model, created_at)
VALUES (1, 'claude', 'fixture-claude', 'fixture-model', '2026-01-01T00:00:00Z');

INSERT INTO usage_daily_rollups (date, app_type, provider_id, request_count)
VALUES ('2026-01-01', 'claude', 'fixture-claude', 1);

PRAGMA user_version = 16;
