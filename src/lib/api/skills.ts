import { invoke } from "@tauri-apps/api/core";

import type { ManagedAppId, ManagedClientApps } from "@/lib/api/types";

export type AppType = ManagedAppId;
export type SkillApps = ManagedClientApps;

/** Metadata for one locally managed Skill. Content remains in WSL live folders. */
export interface InstalledSkill {
  id: string;
  name: string;
  description?: string;
  directory: string;
  contentHash?: string;
  totalSizeBytes: number;
  fileCount: number;
  apps: SkillApps;
  cloudEligible: boolean;
  createdAtMs: number;
  updatedAtMs: number;
}

/** One live copy of an unmanaged Skill: client and its content digest. */
export interface UnmanagedSkillCopy {
  client: ManagedAppId;
  contentHash: string;
}

export interface UnmanagedSkill {
  directory: string;
  name: string;
  description?: string;
  foundIn: ManagedAppId[];
  /** Per-client content digests from the scan; absent falls back to source-only defaults. */
  copies?: UnmanagedSkillCopy[];
  path: string;
}

export interface ImportSkillSelection {
  directory: string;
  sourceClient: ManagedAppId;
  apps: SkillApps;
}

/** Read-only SKILL.md preview payload for the skill detail dialog. */
export interface SkillDocumentRead {
  sourceClient: ManagedAppId;
  sizeBytes: number;
  content: string;
}

export const skillsApi = {
  async getInstalled(): Promise<InstalledSkill[]> {
    return await invoke("get_installed_skills");
  },

  async uninstallUnified(id: string): Promise<boolean> {
    return await invoke("uninstall_skill_unified", { id });
  },

  async toggleApp(
    id: string,
    app: ManagedAppId,
    sourceApp: ManagedAppId,
    enabled: boolean,
  ): Promise<InstalledSkill> {
    return await invoke("toggle_skill_app", { id, app, sourceApp, enabled });
  },

  async syncFromLive(
    id: string,
    sourceApp: ManagedAppId,
  ): Promise<InstalledSkill> {
    return await invoke("sync_skill_from_live", { id, sourceApp });
  },

  async scanUnmanaged(): Promise<UnmanagedSkill[]> {
    return await invoke("scan_unmanaged_skills");
  },

  async importFromApps(
    imports: ImportSkillSelection[],
  ): Promise<InstalledSkill[]> {
    return await invoke("import_skills_from_apps", { imports });
  },

  async readDocument(id: string): Promise<SkillDocumentRead> {
    return await invoke("read_skill_document", { id });
  },

  async openDirectory(id: string, app: ManagedAppId): Promise<boolean> {
    return await invoke("open_skill_directory", { id, app });
  },
};
