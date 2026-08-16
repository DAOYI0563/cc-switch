import { useEffect, useState } from "react";
import { settingsApi } from "@/lib/api";

export interface UseSettingsMetadataResult {
  isPortable: boolean;
  isLoading: boolean;
}

/**
 * useSettingsMetadata - 元数据管理
 * 负责：
 * - isPortable（便携模式）
 */
export function useSettingsMetadata(): UseSettingsMetadataResult {
  const [isPortable, setIsPortable] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  // 加载元数据
  useEffect(() => {
    let active = true;
    setIsLoading(true);

    const load = async () => {
      try {
        const portable = await settingsApi.isPortable();

        if (!active) return;

        setIsPortable(portable);
      } catch (error) {
        console.error("[useSettingsMetadata] Failed to load metadata", error);
      } finally {
        if (active) {
          setIsLoading(false);
        }
      }
    };

    void load();
    return () => {
      active = false;
    };
  }, []);

  return {
    isPortable,
    isLoading,
  };
}
