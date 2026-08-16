import type { ManagedAppId } from "@/lib/api/types";
import type {
  McpServer,
  Provider,
  SessionMessage,
  SessionMeta,
  Settings,
} from "@/types";
import { deepClone } from "@/utils/deepClone";

type ProvidersByApp = Record<ManagedAppId, Record<string, Provider>>;
type CurrentProviderState = Record<ManagedAppId, string>;
type McpConfigState = Record<ManagedAppId, Record<string, McpServer>>;

const defaultProviders = (): ProvidersByApp => ({
  claude: {
    "claude-1": {
      id: "claude-1",
      name: "Claude Default",
      settingsConfig: {},
      category: "official",
      sortIndex: 0,
      createdAt: Date.now(),
    },
    "claude-2": {
      id: "claude-2",
      name: "Claude Custom",
      settingsConfig: {},
      category: "custom",
      sortIndex: 1,
      createdAt: Date.now() + 1,
    },
  },
  codex: {
    "codex-1": {
      id: "codex-1",
      name: "Codex Default",
      settingsConfig: {},
      category: "official",
      sortIndex: 0,
      createdAt: Date.now(),
    },
    "codex-2": {
      id: "codex-2",
      name: "Codex Secondary",
      settingsConfig: {},
      category: "custom",
      sortIndex: 1,
      createdAt: Date.now() + 1,
    },
  },
  opencode: {},
});

const defaultCurrent = (): CurrentProviderState => ({
  claude: "claude-1",
  codex: "codex-1",
  opencode: "",
});

const defaultSettings = (): Settings => ({
  showInTray: true,
  language: "zh",
});

const defaultMcp = (): McpConfigState => ({
  claude: {
    sample: {
      id: "sample",
      name: "Sample Claude Server",
      enabled: true,
      apps: { claude: true, codex: false, opencode: false },
      server: { type: "stdio", command: "claude-server" },
    },
  },
  codex: {
    httpServer: {
      id: "httpServer",
      name: "HTTP Codex Server",
      enabled: false,
      apps: { claude: false, codex: true, opencode: false },
      server: { type: "http", url: "http://localhost:3000" },
    },
  },
  opencode: {},
});

const messageKey = (providerId: string, sessionId: string) =>
  `${providerId}:${sessionId}`;

const defaultSessions = (): SessionMeta[] => {
  const now = Date.now();
  return [
    {
      providerId: "codex",
      sessionId: "codex-session-1",
      title: "Codex Session One",
      summary: "Codex summary",
      projectDir: "/mock/codex",
      createdAt: now - 2000,
      lastActiveAt: now - 1000,
      resumeCommand: "codex resume codex-session-1",
    },
    {
      providerId: "claude",
      sessionId: "claude-session-1",
      title: "Claude Session One",
      summary: "Claude summary",
      projectDir: "/mock/claude",
      createdAt: now - 4000,
      lastActiveAt: now - 3000,
      resumeCommand: "claude --resume claude-session-1",
    },
  ];
};

const defaultMessages = (): Record<string, SessionMessage[]> => ({
  [messageKey("codex", "codex-session-1")]: [
    {
      sequence: 0,
      role: "user",
      content: "First codex message",
      occurredAt: Date.now() - 1000,
    },
  ],
  [messageKey("claude", "claude-session-1")]: [
    {
      sequence: 0,
      role: "user",
      content: "First claude message",
      occurredAt: Date.now() - 3000,
    },
  ],
});

let providers = defaultProviders();
let current = defaultCurrent();
let settings = defaultSettings();
let mcpConfigs = defaultMcp();
let sessions = defaultSessions();
let messages = defaultMessages();
let openCodeLiveProviderIds: string[] = [];

export const resetProviderState = () => {
  providers = defaultProviders();
  current = defaultCurrent();
  settings = defaultSettings();
  mcpConfigs = defaultMcp();
  sessions = defaultSessions();
  messages = defaultMessages();
  openCodeLiveProviderIds = [];
};

export const getProviders = (app: ManagedAppId) =>
  deepClone(providers[app]) as Record<string, Provider>;

export const getCurrentProviderId = (app: ManagedAppId) => current[app];

export const setCurrentProviderId = (app: ManagedAppId, id: string) => {
  current[app] = id;
};

export const getOpenCodeLiveProviderIds = () => [...openCodeLiveProviderIds];

export const addProvider = (app: ManagedAppId, provider: Provider) => {
  providers[app][provider.id] = deepClone(provider) as Provider;
};

export const updateProvider = (app: ManagedAppId, provider: Provider) => {
  providers[app][provider.id] = {
    ...providers[app][provider.id],
    ...deepClone(provider),
  };
};

export const deleteProvider = (app: ManagedAppId, id: string) => {
  delete providers[app][id];
  if (current[app] === id) current[app] = Object.keys(providers[app])[0] ?? "";
};

export const updateSortOrder = (
  app: ManagedAppId,
  updates: { id: string; sortIndex: number }[],
) => {
  for (const { id, sortIndex } of updates) {
    if (providers[app][id]) providers[app][id].sortIndex = sortIndex;
  }
};

export const listProviders = (app: ManagedAppId) => getProviders(app);

export const getSettings = () => deepClone(settings) as Settings;

export const setSettings = (updates: Partial<Settings>) => {
  settings = { ...settings, ...deepClone(updates) };
};

export const getMcpConfig = (app: ManagedAppId) => ({
  configPath: `/mock/${app}.mcp.json`,
  servers: deepClone(mcpConfigs[app]) as Record<string, McpServer>,
});

export const setMcpServerEnabled = (
  app: ManagedAppId,
  id: string,
  enabled: boolean,
) => {
  if (mcpConfigs[app][id]) mcpConfigs[app][id].enabled = enabled;
};

export const upsertMcpServer = (
  app: ManagedAppId,
  id: string,
  server: McpServer,
) => {
  mcpConfigs[app][id] = deepClone(server) as McpServer;
};

export const deleteMcpServer = (app: ManagedAppId, id: string) => {
  delete mcpConfigs[app][id];
};

export const listSessions = () => deepClone(sessions) as SessionMeta[];

export const getSessionMessages = (providerId: string, sessionId: string) =>
  deepClone(messages[messageKey(providerId, sessionId)] ?? []) as SessionMessage[];
