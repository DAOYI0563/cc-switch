import { invoke } from "@tauri-apps/api/core";

import type {
  DailyBriefRecord,
  DailyBriefSettingsView,
  SaveDailyBriefSettingsRequest,
} from "@/types";

export const dailyBriefApi = {
  async getSettings(): Promise<DailyBriefSettingsView> {
    return await invoke("get_daily_brief_settings");
  },

  async saveSettings(
    request: SaveDailyBriefSettingsRequest,
  ): Promise<DailyBriefSettingsView> {
    return await invoke("save_daily_brief_settings_command", { request });
  },

  async testConnection(): Promise<DailyBriefSettingsView> {
    return await invoke("test_daily_brief_connection");
  },

  async list(query?: string): Promise<DailyBriefRecord[]> {
    return await invoke("list_daily_briefs", { query });
  },

  async generate(date: string, regenerate = false): Promise<DailyBriefRecord> {
    return await invoke("generate_daily_brief", { date, regenerate });
  },

  async delete(date: string, deviceId: string): Promise<void> {
    await invoke("delete_daily_brief", { date, deviceId });
  },

  async open(date: string, deviceId: string): Promise<void> {
    await invoke("open_daily_brief", { date, deviceId });
  },

  async openDirectory(): Promise<void> {
    await invoke("open_daily_brief_directory");
  },
};
