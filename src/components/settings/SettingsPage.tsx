import { useCallback, useEffect, useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";

import { AboutSection } from "@/components/settings/AboutSection";
import { DirectorySettings } from "@/components/settings/DirectorySettings";
import { ThemeSettings } from "@/components/settings/ThemeSettings";
import { WebdavSyncSection } from "@/components/settings/WebdavSyncSection";
import { WindowSettings } from "@/components/settings/WindowSettings";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useSettings, type SettingsFormState } from "@/hooks/useSettings";

interface SettingsPageProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onImportSuccess?: () => void | Promise<void>;
  defaultTab?: string;
}

const RETAINED_TABS = ["general", "webdav", "paths", "about"] as const;

export function SettingsPage({
  open,
  onOpenChange: _onOpenChange,
  defaultTab = "general",
}: SettingsPageProps) {
  const {
    settings,
    isLoading,
    isPortable,
    resolvedDirs,
    updateSettings,
    autoSaveSettings,
  } = useSettings();
  const normalizedDefault = RETAINED_TABS.includes(
    defaultTab as (typeof RETAINED_TABS)[number],
  )
    ? defaultTab
    : "general";
  const [activeTab, setActiveTab] = useState(normalizedDefault);

  useEffect(() => {
    if (open) setActiveTab(normalizedDefault);
  }, [normalizedDefault, open]);

  const autoSave = useCallback(
    async (updates: Partial<SettingsFormState>) => {
      if (!settings) return;
      const previous = Object.fromEntries(
        Object.keys(updates).map((key) => [
          key,
          settings[key as keyof SettingsFormState],
        ]),
      ) as Partial<SettingsFormState>;
      updateSettings(updates);
      try {
        await autoSaveSettings(updates);
      } catch {
        updateSettings(previous);
        toast.error("设置保存失败");
      }
    },
    [autoSaveSettings, settings, updateSettings],
  );

  return (
    <div className="flex h-full flex-col overflow-hidden px-6">
      {isLoading && !settings ? (
        <div className="flex flex-1 items-center justify-center">
          <Loader2 className="h-7 w-7 animate-spin text-muted-foreground" />
        </div>
      ) : (
        <Tabs
          value={activeTab}
          onValueChange={setActiveTab}
          className="flex h-full flex-col"
        >
          <TabsList className="mb-5 grid w-full grid-cols-4 rounded-lg">
            <TabsTrigger value="general">通用</TabsTrigger>
            <TabsTrigger value="webdav">WebDAV</TabsTrigger>
            <TabsTrigger value="paths">WSL 路径</TabsTrigger>
            <TabsTrigger value="about">关于</TabsTrigger>
          </TabsList>

          <div className="min-h-0 flex-1 overflow-y-auto pr-2">
            <TabsContent
              value="general"
              className="mx-auto mt-0 max-w-3xl space-y-8 pb-8"
            >
              {settings ? (
                <>
                  <ThemeSettings />
                  <WindowSettings settings={settings} onChange={autoSave} />
                  <section className="border-t border-border-default pt-5">
                    <h3 className="text-sm font-medium">运行日志</h3>
                    <p className="mt-1 text-xs leading-relaxed text-muted-foreground">
                      应用固定保留滚动的 info
                      级别日志；凭据、令牌、会话正文和远端响应正文不会写入日志。
                    </p>
                  </section>
                </>
              ) : null}
            </TabsContent>

            <TabsContent value="webdav" className="mx-auto mt-0 max-w-4xl pb-8">
              <WebdavSyncSection config={settings?.webdavSync} />
            </TabsContent>

            <TabsContent
              value="paths"
              className="mx-auto mt-0 max-w-4xl space-y-5 pb-8"
            >
              {settings ? (
                <>
                  <DirectorySettings resolvedDirs={resolvedDirs} />
                </>
              ) : null}
            </TabsContent>

            <TabsContent value="about" className="mx-auto mt-0 max-w-4xl pb-8">
              <AboutSection isPortable={isPortable} />
            </TabsContent>
          </div>
        </Tabs>
      )}
    </div>
  );
}
