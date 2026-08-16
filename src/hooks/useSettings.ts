import { useCallback, useMemo } from "react";
import { toast } from "sonner";

import { providersApi, settingsApi } from "@/lib/api";
import { useSaveSettingsMutation, useSettingsQuery } from "@/lib/query";
import type { Settings } from "@/types";
import { useSettingsForm, type SettingsFormState } from "./useSettingsForm";
import { useSettingsMetadata } from "./useSettingsMetadata";

export interface ResolvedDirectories {
  claude: string;
  codex: string;
  opencode: string;
}

export interface UseSettingsResult {
  settings: SettingsFormState | null;
  isLoading: boolean;
  isSaving: boolean;
  isPortable: boolean;
  resolvedDirs: ResolvedDirectories;
  updateSettings: (updates: Partial<SettingsFormState>) => void;
  saveSettings: (
    overrides?: Partial<SettingsFormState>,
    options?: { silent?: boolean },
  ) => Promise<boolean>;
  autoSaveSettings: (updates: Partial<SettingsFormState>) => Promise<boolean>;
  resetSettings: () => void;
}

export type { SettingsFormState };

const FIXED_DIRECTORIES: ResolvedDirectories = {
  claude: String.raw`\\wsl.localhost\Ubuntu\home\zhldm\.claude`,
  codex: String.raw`\\wsl.localhost\Ubuntu\home\zhldm\.codex`,
  opencode: String.raw`\\wsl.localhost\Ubuntu\home\zhldm\.config\opencode`,
};

export function useSettings(): UseSettingsResult {
  const { data } = useSettingsQuery();
  const saveMutation = useSaveSettingsMutation();
  const {
    settings,
    isLoading: isFormLoading,
    updateSettings,
    resetSettings,
  } = useSettingsForm();
  const { isPortable, isLoading: isMetadataLoading } = useSettingsMetadata();

  const persist = useCallback(
    async (
      updates?: Partial<SettingsFormState>,
      options?: { silent?: boolean },
    ) => {
      if (!settings) return false;
      const payload: Settings = { ...settings, ...updates, language: "zh" };
      await saveMutation.mutateAsync(payload);

      if (
        updates?.launchOnStartup !== undefined &&
        updates.launchOnStartup !== data?.launchOnStartup
      ) {
        await settingsApi.setAutoLaunch(updates.launchOnStartup);
      }
      await providersApi.updateTrayMenu().catch(() => undefined);
      if (!options?.silent) toast.success("设置已保存");
      return true;
    },
    [data?.launchOnStartup, saveMutation, settings],
  );

  return {
    settings,
    isLoading: useMemo(
      () => isFormLoading || isMetadataLoading,
      [isFormLoading, isMetadataLoading],
    ),
    isSaving: saveMutation.isPending,
    isPortable,
    resolvedDirs: FIXED_DIRECTORIES,
    updateSettings,
    saveSettings: persist,
    autoSaveSettings: (updates) => persist(updates, { silent: true }),
    resetSettings: () => resetSettings(data ?? null),
  };
}
