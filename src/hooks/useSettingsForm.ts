import { useCallback, useEffect, useRef, useState } from "react";

import { useSettingsQuery } from "@/lib/query";
import type { Settings } from "@/types";

export type SettingsFormState = Settings & { language: "zh" };

export interface UseSettingsFormResult {
  settings: SettingsFormState | null;
  isLoading: boolean;
  updateSettings: (updates: Partial<SettingsFormState>) => void;
  resetSettings: (serverData: Settings | null) => void;
}

function normalize(data: Settings): SettingsFormState {
  return {
    showInTray: data.showInTray ?? true,
    useAppWindowControls: data.useAppWindowControls ?? true,
    launchOnStartup: data.launchOnStartup ?? false,
    silentStartup: data.silentStartup ?? false,
    webdavSync: data.webdavSync,
    language: "zh",
  };
}

export function useSettingsForm(): UseSettingsFormResult {
  const { data, isLoading } = useSettingsQuery();
  const [settings, setSettings] = useState<SettingsFormState | null>(null);
  const dirty = useRef(false);

  useEffect(() => {
    if (data && !dirty.current) setSettings(normalize(data));
  }, [data]);

  const updateSettings = useCallback((updates: Partial<SettingsFormState>) => {
    dirty.current = true;
    setSettings((current) =>
      current ? { ...current, ...updates, language: "zh" } : current,
    );
  }, []);

  const resetSettings = useCallback((serverData: Settings | null) => {
    if (!serverData) return;
    dirty.current = false;
    setSettings(normalize(serverData));
  }, []);

  return { settings, isLoading, updateSettings, resetSettings };
}
