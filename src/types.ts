import type { ManagedClientApps } from "@/lib/api/types";

export type ProviderCategory = "official" | "custom";

export interface ProviderMeta {
  commonConfigEnabled?: boolean;
  isFullUrl?: boolean;
  customUserAgent?: string;
}

export interface Provider {
  id: string;
  name: string;
  settingsConfig: Record<string, unknown>;
  websiteUrl?: string;
  category?: ProviderCategory;
  createdAt?: number;
  sortIndex?: number;
  notes?: string;
  meta?: ProviderMeta;
  icon?: string;
  iconColor?: string;
}

export interface WebDavSyncSettings {
  baseUrl?: string;
  username?: string;
  password?: string;
  remoteRoot?: string;
  profile?: string;
}

export interface Settings {
  showInTray: boolean;
  useAppWindowControls?: boolean;
  launchOnStartup?: boolean;
  silentStartup?: boolean;
  language?: "zh";
  webdavSync?: WebDavSyncSettings;
}

export interface SyncFirstSyncChangeCounts {
  additions: number;
  modifications: number;
  deletions: number;
  conflicts: number;
}

export interface SyncFirstSyncPreview {
  schemaVersion: 1;
  candidateDeviceId: string;
  displayName: string;
  observedAtMs: number;
  remoteGeneration: number;
  remoteEtag?: string;
  remoteManifestSha256: string;
  localStateSha256: string;
  changes: SyncFirstSyncChangeCounts;
  previewToken: string;
}

export interface SyncDevice {
  schemaVersion: 1;
  deviceId: string;
  displayName: string;
  acknowledgedGeneration: number;
  registeredAtMs: number;
  lastSeenAtMs: number;
  status: "active" | "retired";
  retiredAtMs?: number;
}

export interface SyncRunResult {
  schemaVersion: 1;
  committedGeneration: number;
  attempts: number;
  resolvedRecords: number;
  conflicts: number;
  committedEtag?: string;
}

export interface SessionMeta {
  providerId: string;
  sessionId: string;
  title?: string;
  summary?: string;
  projectDir?: string | null;
  createdAt?: number;
  lastActiveAt?: number;
  resumeCommand?: string;
}

export interface SessionMessage {
  sequence: number;
  role: string;
  content: string;
  occurredAt?: number;
}

export interface SessionPage<T> {
  items: T[];
  offset: number;
  total: number;
  nextOffset?: number;
}

export interface SessionSearchRequest {
  providerId?: "all" | "claude" | "codex" | "opencode";
  project?: string;
  fromMs?: number;
  toMs?: number;
  keyword?: string;
  offset?: number;
  limit?: number;
}

export interface CliStatus {
  id: "claude" | "codex" | "opencode";
  displayName: string;
  currentVersion?: string;
  latestVersion?: string;
  installationChannel: string;
  executablePath?: string;
  latestSourceUrl: string;
  wslCommand: string;
  powershellCommand: string;
  state: "ok" | "notInstalled" | "latestUnavailable" | "currentUnavailable";
  detail?: string;
}

export interface DailyBriefSettings {
  apiUrl: string;
  model: string;
  focus: string;
  autoEnabled: boolean;
  enabledAtMs?: number;
  privacyConfirmationHash?: string;
  connectionTestHash?: string;
}

export interface DailyBriefSettingsView extends DailyBriefSettings {
  hasApiKey: boolean;
}

export interface SaveDailyBriefSettingsRequest {
  apiUrl: string;
  model: string;
  focus: string;
  autoEnabled: boolean;
  apiKey?: string;
  confirmPrivacy: boolean;
}

export interface DailyBriefRecord {
  date: string;
  deviceId: string;
  status:
    | "disabled"
    | "pending"
    | "waiting_for_stability"
    | "running"
    | "pending_resume"
    | "complete"
    | "failed"
    | "no_sessions"
    | "integrity_invalid";
  sourceFingerprint?: string;
  contentHash?: string;
  localPath?: string;
  sourceState: "present" | "changed" | "missing";
  modelName?: string;
  templateVersion?: string;
  promptVersion?: string;
  generatedAtMs?: number;
  updatedAtMs: number;
}

export interface McpServerSpec {
  type?: "stdio" | "http" | "sse";
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  cwd?: string;
  url?: string;
  headers?: Record<string, string>;
  [key: string]: unknown;
}

export type McpApps = ManagedClientApps;

export interface McpServer {
  id: string;
  name: string;
  server: McpServerSpec;
  apps: McpApps;
  description?: string;
  tags?: string[];
  homepage?: string;
  docs?: string;
  enabled?: boolean;
  source?: string;
}

export type McpServersMap = Record<string, McpServer>;
