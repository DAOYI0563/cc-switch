import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Provider } from "@/types";
import type { ManagedAppId } from "./types";

export interface ProviderSortUpdate {
  id: string;
  sortIndex: number;
}

export interface ProviderSwitchEvent {
  appType: ManagedAppId;
  providerId: string;
}

export interface SwitchResult {
  warnings: string[];
}

export const providersApi = {
  async getAll(appId: ManagedAppId): Promise<Record<string, Provider>> {
    return await invoke("get_providers", { app: appId });
  },
  async getCurrent(appId: ManagedAppId): Promise<string> {
    return await invoke("get_current_provider", { app: appId });
  },
  async add(
    provider: Provider,
    appId: ManagedAppId,
    addToLive?: boolean,
  ): Promise<boolean> {
    return await invoke("add_provider", { provider, app: appId, addToLive });
  },
  async update(
    provider: Provider,
    appId: ManagedAppId,
    originalId?: string,
  ): Promise<boolean> {
    return await invoke("update_provider", {
      provider,
      app: appId,
      originalId,
    });
  },
  async delete(id: string, appId: ManagedAppId): Promise<boolean> {
    return await invoke("delete_provider", { id, app: appId });
  },
  async removeFromLiveConfig(
    id: string,
    appId: ManagedAppId,
  ): Promise<boolean> {
    return await invoke("remove_provider_from_live_config", { id, app: appId });
  },
  async switch(id: string, appId: ManagedAppId): Promise<SwitchResult> {
    return await invoke("switch_provider", { id, app: appId });
  },
  async importDefault(appId: ManagedAppId): Promise<boolean> {
    return await invoke("import_default_config", { app: appId });
  },
  async updateTrayMenu(): Promise<boolean> {
    return await invoke("update_tray_menu");
  },
  async updateSortOrder(
    updates: ProviderSortUpdate[],
    appId: ManagedAppId,
  ): Promise<boolean> {
    return await invoke("update_providers_sort_order", { updates, app: appId });
  },
  async onSwitched(
    handler: (event: ProviderSwitchEvent) => void,
  ): Promise<UnlistenFn> {
    return await listen("provider-switched", (event) =>
      handler(event.payload as ProviderSwitchEvent),
    );
  },
  async openTerminal(
    providerId: string,
    appId: ManagedAppId,
    options?: { cwd?: string },
  ): Promise<boolean> {
    return await invoke("open_provider_terminal", {
      providerId,
      app: appId,
      cwd: options?.cwd,
    });
  },
  async importOpenCodeFromLive(): Promise<number> {
    return await invoke("import_opencode_providers_from_live");
  },
  async getOpenCodeLiveProviderIds(): Promise<string[]> {
    return await invoke("get_opencode_live_provider_ids");
  },
};
