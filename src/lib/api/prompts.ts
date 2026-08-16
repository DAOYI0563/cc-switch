import { invoke } from "@tauri-apps/api/core";
import type { ManagedAppId } from "./types";

export interface Prompt {
  id: string;
  name: string;
  version: number;
  content: string;
  description?: string;
  enabled: boolean;
  createdAt?: number;
  updatedAt?: number;
}

export const PROMPT_LIVE_FILENAMES: Record<ManagedAppId, string> = {
  claude: "CLAUDE.md",
  codex: "AGENTS.md",
  opencode: "AGENTS.md",
};

export const promptsApi = {
  async getPrompts(app: ManagedAppId): Promise<Record<string, Prompt>> {
    return await invoke("get_prompts", { app });
  },

  async upsertPrompt(
    app: ManagedAppId,
    id: string,
    prompt: Prompt,
  ): Promise<Prompt> {
    return await invoke("upsert_prompt", { app, id, prompt });
  },

  async deletePrompt(app: ManagedAppId, id: string): Promise<void> {
    return await invoke("delete_prompt", { app, id });
  },

  async enablePrompt(app: ManagedAppId, id: string): Promise<void> {
    return await invoke("enable_prompt", { app, id });
  },

  async importFromFile(app: ManagedAppId): Promise<string> {
    return await invoke("import_prompt_from_file", { app });
  },

  async getCurrentFileContent(app: ManagedAppId): Promise<string | null> {
    return await invoke("get_current_prompt_file_content", { app });
  },

  async syncToLive(app: ManagedAppId): Promise<void> {
    return await invoke("sync_prompt_to_live", { app });
  },
};
