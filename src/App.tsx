import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Book,
  CalendarClock,
  FolderInput,
  History,
  Maximize2,
  Minimize2,
  Minus,
  Plus,
  Settings,
  TriangleAlert,
  Upload,
  Wrench,
  X,
} from "lucide-react";
import { toast } from "sonner";

import { AppSwitcher } from "@/components/AppSwitcher";
import { McpIcon } from "@/components/BrandIcons";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { ConflictCenterPage } from "@/components/conflicts/ConflictCenterPage";
import { DailyBriefPage } from "@/components/daily-brief/DailyBriefPage";
import UnifiedMcpPanel from "@/components/mcp/UnifiedMcpPanel";
import { AddProviderDialog } from "@/components/providers/AddProviderDialog";
import { EditProviderDialog } from "@/components/providers/EditProviderDialog";
import { ProviderList } from "@/components/providers/ProviderList";
import PromptPanel, {
  type PromptPanelHandle,
} from "@/components/prompts/PromptPanel";
import { SessionManagerPage } from "@/components/sessions/SessionManagerPage";
import { SettingsPage } from "@/components/settings/SettingsPage";
import UnifiedSkillsPanel, {
  type UnifiedSkillsPanelHandle,
} from "@/components/skills/UnifiedSkillsPanel";
import { Button } from "@/components/ui/button";
import { useLocalScanPage } from "@/hooks/useLocalScanPage";
import { useProviderActions } from "@/hooks/useProviderActions";
import {
  providersApi,
  readStoredManagedAppId,
  settingsApi,
  type ManagedAppId,
} from "@/lib/api";
import { useProvidersQuery } from "@/lib/query";
import { cn } from "@/lib/utils";
import type { Provider } from "@/types";
import { deepClone } from "@/utils/deepClone";
import { extractErrorMessage } from "@/utils/errorUtils";

type View =
  | "providers"
  | "mcp"
  | "prompts"
  | "skills"
  | "conflicts"
  | "sessions"
  | "briefs"
  | "settings";

const APP_STORAGE_KEY = "wsl-code-switch:last-client";
const VIEW_STORAGE_KEY = "wsl-code-switch:last-view";
const VIEWS: View[] = [
  "providers",
  "mcp",
  "prompts",
  "skills",
  "conflicts",
  "sessions",
  "briefs",
  "settings",
];

const initialClient = (): ManagedAppId =>
  readStoredManagedAppId(localStorage, APP_STORAGE_KEY);

const initialView = (): View => {
  const stored = localStorage.getItem(VIEW_STORAGE_KEY) as View | null;
  return stored && VIEWS.includes(stored) ? stored : "providers";
};

function App() {
  const queryClient = useQueryClient();
  const [activeApp, setActiveApp] = useState<ManagedAppId>(initialClient);
  const [currentView, setCurrentView] = useState<View>(initialView);
  const [isAddOpen, setIsAddOpen] = useState(false);
  const [editingProvider, setEditingProvider] = useState<Provider | null>(null);
  const [confirmAction, setConfirmAction] = useState<{
    provider: Provider;
    action: "delete" | "remove";
  } | null>(null);
  const [isMaximized, setIsMaximized] = useState(false);
  const promptRef = useRef<PromptPanelHandle>(null);
  const mcpRef = useRef<{
    openImport: () => void;
    syncToApps: () => void;
    openAdd: () => void;
  } | null>(null);
  const skillsRef = useRef<UnifiedSkillsPanelHandle>(null);

  const scanDomain =
    currentView === "providers"
      ? "provider"
      : currentView === "mcp"
        ? "mcp"
        : currentView === "prompts"
          ? "prompt"
          : currentView === "skills"
            ? "skill"
            : null;
  useLocalScanPage(scanDomain);

  useEffect(
    () => localStorage.setItem(VIEW_STORAGE_KEY, currentView),
    [currentView],
  );
  useEffect(
    () => localStorage.setItem(APP_STORAGE_KEY, activeApp),
    [activeApp],
  );

  const providerQuery = useProvidersQuery(activeApp);
  const providers = useMemo(
    () => providerQuery.data?.providers ?? {},
    [providerQuery.data],
  );
  const currentProviderId = providerQuery.data?.currentProviderId ?? "";
  const { addProvider, updateProvider, switchProvider, deleteProvider } =
    useProviderActions(activeApp);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    void providersApi
      .onSwitched(async (event) => {
        if (active && event.appType === activeApp)
          await providerQuery.refetch();
      })
      .then((listener) => {
        if (active) dispose = listener;
        else listener();
      })
      .catch(() => undefined);
    return () => {
      active = false;
      dispose?.();
    };
  }, [activeApp, providerQuery.refetch]);

  useEffect(() => {
    let active = true;
    let dispose: (() => void) | undefined;
    const window = getCurrentWindow();
    const update = async () => {
      const maximized = await window.isMaximized();
      if (active) setIsMaximized(maximized);
    };
    void update();
    void window
      .onResized(() => void update())
      .then((listener) => {
        if (active) dispose = listener;
        else listener();
      });
    return () => {
      active = false;
      dispose?.();
    };
  }, []);

  const editProvider = async ({
    provider,
    originalId,
  }: {
    provider: Provider;
    originalId?: string;
  }) => {
    await updateProvider(provider, originalId);
    setEditingProvider(null);
  };

  const duplicateProvider = async (provider: Provider) => {
    const keys = new Set(Object.keys(providers));
    let providerKey = `${provider.id}-copy`;
    let index = 2;
    while (keys.has(providerKey))
      providerKey = `${provider.id}-copy-${index++}`;
    await addProvider({
      name: `${provider.name} 副本`,
      providerKey: activeApp === "opencode" ? providerKey : undefined,
      addToLive: false,
      settingsConfig: deepClone(provider.settingsConfig),
      websiteUrl: provider.websiteUrl,
      category: provider.category,
      notes: provider.notes,
      meta: provider.meta ? deepClone(provider.meta) : undefined,
      icon: provider.icon,
      iconColor: provider.iconColor,
      sortIndex:
        provider.sortIndex === undefined ? undefined : provider.sortIndex + 1,
    });
  };

  const confirmProviderAction = async () => {
    if (!confirmAction) return;
    if (confirmAction.action === "delete") {
      await deleteProvider(confirmAction.provider.id);
    } else {
      await providersApi.removeFromLiveConfig(
        confirmAction.provider.id,
        activeApp,
      );
      await queryClient.invalidateQueries({
        queryKey: ["opencodeLiveProviderIds"],
      });
      toast.success("已从 OpenCode live 配置移出");
    }
    setConfirmAction(null);
  };

  const openWebsite = async (url: string) => {
    try {
      await settingsApi.openExternal(url);
    } catch (error) {
      toast.error(extractErrorMessage(error) || "无法打开链接");
    }
  };

  const openTerminal = async (provider: Provider) => {
    try {
      const cwd = await settingsApi.pickDirectory();
      if (cwd) await providersApi.openTerminal(provider.id, activeApp, { cwd });
    } catch (error) {
      toast.error(extractErrorMessage(error) || "无法打开终端");
    }
  };

  const renderPage = () => {
    switch (currentView) {
      case "mcp":
        return (
          <UnifiedMcpPanel
            ref={mcpRef}
            onOpenChange={() => setCurrentView("providers")}
          />
        );
      case "prompts":
        return (
          <PromptPanel
            ref={promptRef}
            open
            appId={activeApp}
            onOpenChange={() => setCurrentView("providers")}
          />
        );
      case "skills":
        return <UnifiedSkillsPanel ref={skillsRef} currentApp={activeApp} />;
      case "conflicts":
        return <ConflictCenterPage />;
      case "sessions":
        return <SessionManagerPage appId={activeApp} />;
      case "briefs":
        return <DailyBriefPage />;
      case "settings":
        return (
          <SettingsPage open onOpenChange={() => setCurrentView("providers")} />
        );
      default:
        return (
          <div className="h-full overflow-y-auto px-6 pb-10">
            <ProviderList
              providers={providers}
              currentProviderId={currentProviderId}
              appId={activeApp}
              isLoading={providerQuery.isLoading}
              onSwitch={(provider) => void switchProvider(provider)}
              onEdit={setEditingProvider}
              onDelete={(provider) =>
                setConfirmAction({ provider, action: "delete" })
              }
              onRemoveFromConfig={(provider) =>
                setConfirmAction({ provider, action: "remove" })
              }
              onDuplicate={(provider) => void duplicateProvider(provider)}
              onOpenWebsite={(url) => void openWebsite(url)}
              onOpenTerminal={
                activeApp === "claude"
                  ? (provider) => void openTerminal(provider)
                  : undefined
              }
              onCreate={() => setIsAddOpen(true)}
            />
          </div>
        );
    }
  };

  const title: Record<View, string> = {
    providers: "供应商",
    mcp: "MCP",
    prompts: "Prompt",
    skills: "Skill",
    conflicts: "冲突中心",
    sessions: "会话",
    briefs: "每日简报",
    settings: "设置",
  };

  return (
    <div className="flex h-screen flex-col overflow-hidden bg-background text-foreground">
      <header
        className="h-16 shrink-0 border-b border-border-default"
        data-tauri-drag-region
      >
        <div
          className="flex h-full items-center gap-3 px-4"
          data-tauri-drag-region
        >
          <div className="flex min-w-0 items-center gap-2">
            {currentView !== "providers" ? (
              <Button
                variant="ghost"
                size="icon"
                title="返回供应商"
                onClick={() => setCurrentView("providers")}
              >
                <ArrowLeft className="h-4 w-4" />
              </Button>
            ) : null}
            <h1 className="truncate text-base font-semibold">
              {title[currentView]}
            </h1>
          </div>

          <div className="ml-auto flex min-w-0 items-center gap-1">
            {currentView === "providers" ? (
              <>
                <AppSwitcher activeApp={activeApp} onSwitch={setActiveApp} />
                <NavAction
                  label="冲突中心"
                  icon={TriangleAlert}
                  onClick={() => setCurrentView("conflicts")}
                />
                <NavAction
                  label="Skill"
                  icon={Wrench}
                  onClick={() => setCurrentView("skills")}
                />
                <NavAction
                  label="Prompt"
                  icon={Book}
                  onClick={() => setCurrentView("prompts")}
                />
                <NavAction
                  label="会话"
                  icon={History}
                  onClick={() => setCurrentView("sessions")}
                />
                <Button
                  variant="ghost"
                  size="icon"
                  title="MCP"
                  onClick={() => setCurrentView("mcp")}
                >
                  <McpIcon size={16} />
                </Button>
                <NavAction
                  label="每日简报"
                  icon={CalendarClock}
                  onClick={() => setCurrentView("briefs")}
                />
                <NavAction
                  label="设置"
                  icon={Settings}
                  onClick={() => setCurrentView("settings")}
                />
                <Button
                  size="icon"
                  title="添加供应商"
                  onClick={() => setIsAddOpen(true)}
                >
                  <Plus className="h-4 w-4" />
                </Button>
              </>
            ) : null}
            {currentView === "prompts" ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => promptRef.current?.syncToLive()}
              >
                <Upload className="mr-2 h-4 w-4" />
                同步到 WSL
              </Button>
            ) : null}
            {currentView === "mcp" ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => mcpRef.current?.syncToApps()}
              >
                <Upload className="mr-2 h-4 w-4" />
                同步到 WSL
              </Button>
            ) : null}
            {currentView === "skills" ? (
              <Button
                variant="outline"
                size="sm"
                onClick={() => skillsRef.current?.openImport()}
              >
                <FolderInput className="mr-2 h-4 w-4" />
                导入已有
              </Button>
            ) : null}
          </div>

          <div className="ml-2 flex items-center border-l border-border-default pl-2">
            <WindowAction
              label="最小化"
              icon={Minus}
              onClick={() => void getCurrentWindow().minimize()}
            />
            <WindowAction
              label={isMaximized ? "还原" : "最大化"}
              icon={isMaximized ? Minimize2 : Maximize2}
              onClick={() => void getCurrentWindow().toggleMaximize()}
            />
            <WindowAction
              label="关闭"
              icon={X}
              destructive
              onClick={() => void getCurrentWindow().close()}
            />
          </div>
        </div>
      </header>

      <main className="flex min-h-0 flex-1 flex-col overflow-hidden">
        {renderPage()}
      </main>

      <AddProviderDialog
        open={isAddOpen}
        onOpenChange={setIsAddOpen}
        appId={activeApp}
        onSubmit={addProvider}
      />
      <EditProviderDialog
        open={editingProvider !== null}
        provider={editingProvider}
        appId={activeApp}
        onOpenChange={(open) => !open && setEditingProvider(null)}
        onSubmit={editProvider}
      />
      <ConfirmDialog
        isOpen={confirmAction !== null}
        title={
          confirmAction?.action === "delete"
            ? "删除供应商"
            : "移出 OpenCode 配置"
        }
        message={
          confirmAction?.action === "delete"
            ? `确定删除“${confirmAction.provider.name}”吗？`
            : `确定将“${confirmAction?.provider.name ?? ""}”从 live 配置移出吗？数据库记录会保留。`
        }
        confirmText="确认"
        onConfirm={() => void confirmProviderAction()}
        onCancel={() => setConfirmAction(null)}
      />
    </div>
  );
}

function NavAction({
  label,
  icon: Icon,
  onClick,
}: {
  label: string;
  icon: typeof Settings;
  onClick: () => void;
}) {
  return (
    <Button variant="ghost" size="icon" title={label} onClick={onClick}>
      <Icon className="h-4 w-4" />
    </Button>
  );
}

function WindowAction({
  label,
  icon: Icon,
  destructive = false,
  onClick,
}: {
  label: string;
  icon: typeof X;
  destructive?: boolean;
  onClick: () => void;
}) {
  return (
    <Button
      variant="ghost"
      size="icon"
      title={label}
      onClick={onClick}
      className={cn(
        "h-8 w-8",
        destructive && "hover:bg-destructive hover:text-destructive-foreground",
      )}
    >
      <Icon className="h-4 w-4" />
    </Button>
  );
}

export default App;
