import { invoke } from "@tauri-apps/api/core";

import type {
  Settings,
  SyncDevice,
  SyncFirstSyncPreview,
  SyncRunResult,
  WebDavSyncSettings,
} from "@/types";

export const settingsApi = {
  async get(): Promise<Settings> {
    return await invoke("get_settings");
  },
  async save(settings: Settings): Promise<boolean> {
    return await invoke("save_settings", { settings });
  },
  async isPortable(): Promise<boolean> {
    return await invoke("is_portable_mode");
  },
  async pickDirectory(defaultPath?: string): Promise<string | null> {
    return await invoke("pick_directory", { defaultPath });
  },
  async openAppConfigFolder(): Promise<void> {
    await invoke("open_app_config_folder");
  },
  async openExternal(url: string): Promise<void> {
    const parsed = new URL(url);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
      throw new Error("只允许打开 HTTP/HTTPS 链接");
    }
    await invoke("open_external", { url });
  },
  async setAutoLaunch(enabled: boolean): Promise<boolean> {
    return await invoke("set_auto_launch", { enabled });
  },
  async getAutoLaunchStatus(): Promise<boolean> {
    return await invoke("get_auto_launch_status");
  },
  async webdavTestConnection(
    settings: WebDavSyncSettings,
    preserveEmptyPassword = true,
  ): Promise<{ success: boolean; message: string }> {
    return await invoke("webdav_test_connection", {
      settings,
      preserveEmptyPassword,
    });
  },
  async webdavSyncSaveSettings(
    settings: WebDavSyncSettings,
    passwordTouched = false,
  ): Promise<{ success: boolean }> {
    return await invoke("webdav_sync_save_settings", {
      settings,
      passwordTouched,
    });
  },
  async webdavSyncPreviewFirst(request: {
    passphrase: string;
    displayName: string;
    candidateDeviceId?: string;
  }): Promise<SyncFirstSyncPreview> {
    return await invoke("webdav_sync_preview_first", { request });
  },
  async webdavSyncConfirmFirst(request: {
    passphrase: string;
    displayName: string;
    candidateDeviceId: string;
    observedAtMs: number;
    expectedPreviewToken: string;
  }): Promise<SyncRunResult> {
    return await invoke("webdav_sync_confirm_first", { request });
  },
  async webdavSyncNow(passphrase: string): Promise<SyncRunResult> {
    return await invoke("webdav_sync_now", { request: { passphrase } });
  },
  async webdavSyncListDevices(passphrase: string): Promise<SyncDevice[]> {
    return await invoke("webdav_sync_list_devices", {
      request: { passphrase },
    });
  },
  async webdavSyncRetireDevice(
    passphrase: string,
    targetDeviceId: string,
  ): Promise<SyncRunResult> {
    return await invoke("webdav_sync_retire_device", {
      request: {
        passphrase,
        targetDeviceId,
        confirmedTargetDeviceId: targetDeviceId,
      },
    });
  },
};
