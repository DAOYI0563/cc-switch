import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CloudOff,
  Copy,
  FileText,
  FolderInput,
  FolderOpen,
  Loader2,
  Search,
  Sparkles,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { AppCountBar } from "@/components/common/AppCountBar";
import { AppToggleGroup } from "@/components/common/AppToggleGroup";
import { ListItemRow } from "@/components/common/ListItemRow";
import { ManagementListSearch } from "@/components/common/ManagementListSearch";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { TooltipProvider } from "@/components/ui/tooltip";
import { APP_ICON_MAP, SKILLS_APP_IDS } from "@/config/appConfig";
import {
  type ImportSkillSelection,
  type InstalledSkill,
  type UnmanagedSkill,
  useBulkToggleSkillApp,
  useImportSkillsFromApps,
  useInstalledSkills,
  useScanUnmanagedSkills,
  useSyncSkillFromLive,
  useToggleSkillApp,
  useUninstallSkill,
} from "@/hooks/useSkills";
import { skillsApi, type SkillDocumentRead } from "@/lib/api/skills";
import type { ManagedAppId } from "@/lib/api/types";

interface UnifiedSkillsPanelProps {
  currentApp: ManagedAppId;
  onInteractionBlockedChange?: (blocked: boolean) => void;
  onNavigationBlockedChange?: (blocked: boolean) => void;
}

export interface UnifiedSkillsPanelHandle {
  openImport: () => void;
}

const emptyApps = (): ImportSkillSelection["apps"] => ({
  claude: false,
  codex: false,
  opencode: false,
});

const sourceOnlyApps = (
  source: ManagedAppId,
): ImportSkillSelection["apps"] => ({
  ...emptyApps(),
  [source]: true,
});

const contentHashFor = (
  skill: UnmanagedSkill,
  app: ManagedAppId,
): string | undefined =>
  skill.copies?.find((copy) => copy.client === app)?.contentHash;

/** Targets default to the source plus every found copy with an identical hash. */
const defaultAppsFor = (
  skill: UnmanagedSkill,
  source: ManagedAppId,
): ImportSkillSelection["apps"] => {
  const sourceHash = contentHashFor(skill, source);
  if (!skill.copies || !sourceHash) return sourceOnlyApps(source);
  const apps = emptyApps();
  apps[source] = true;
  for (const copy of skill.copies) {
    if (copy.client !== source && copy.contentHash === sourceHash) {
      apps[copy.client] = true;
    }
  }
  return apps;
};

/** null = single copy or digests unavailable; true/false = consistency across copies. */
const copiesConsistent = (skill: UnmanagedSkill): boolean | null => {
  if (!skill.copies || skill.copies.length < 2) return null;
  const [first] = skill.copies;
  return skill.copies.every((copy) => copy.contentHash === first.contentHash);
};

const formatSkillSize = (bytes: number): string => {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
};

const SKILL_LIVE_DIRS: Record<ManagedAppId, string> = {
  claude: "~/.claude/skills",
  codex: "~/.codex/skills",
  opencode: "~/.config/opencode/skills",
};

const uninstallTargets = (skill: InstalledSkill): string[] =>
  SKILLS_APP_IDS.filter((app) => skill.apps[app]).map(
    (app) => `${SKILL_LIVE_DIRS[app]}/${skill.directory}`,
  );

const UnifiedSkillsPanel = React.forwardRef<
  UnifiedSkillsPanelHandle,
  UnifiedSkillsPanelProps
>(
  (
    { currentApp, onInteractionBlockedChange, onNavigationBlockedChange },
    ref,
  ) => {
    const { t } = useTranslation();
    const [searchQuery, setSearchQuery] = useState("");
    const [importDialogOpen, setImportDialogOpen] = useState(false);
    const [scanPending, setScanPending] = useState(false);
    const [writePending, setWritePending] = useState(false);
    const writeLockRef = React.useRef(false);
    const [skillToDelete, setSkillToDelete] = useState<InstalledSkill | null>(
      null,
    );
    const [detailSkill, setDetailSkill] = useState<InstalledSkill | null>(null);

    const {
      data: skills,
      error: installedError,
      isError: installedIsError,
      isLoading,
      refetch: refetchInstalled,
    } = useInstalledSkills();
    const toggleAppMutation = useToggleSkillApp();
    const bulkToggleAppMutation = useBulkToggleSkillApp();
    const syncFromLiveMutation = useSyncSkillFromLive();
    const uninstallMutation = useUninstallSkill();
    const importMutation = useImportSkillsFromApps();
    const { data: unmanagedSkills, refetch: scanUnmanaged } =
      useScanUnmanagedSkills({ enabled: false });

    const mutationPending =
      toggleAppMutation.isPending ||
      bulkToggleAppMutation.isPending ||
      syncFromLiveMutation.isPending ||
      uninstallMutation.isPending ||
      importMutation.isPending;
    const dialogOpen =
      importDialogOpen || skillToDelete !== null || detailSkill !== null;
    const interactionBlocked = writePending || mutationPending || dialogOpen;

    React.useEffect(() => {
      onInteractionBlockedChange?.(interactionBlocked);
      onNavigationBlockedChange?.(interactionBlocked);
    }, [
      interactionBlocked,
      onInteractionBlockedChange,
      onNavigationBlockedChange,
    ]);

    React.useEffect(
      () => () => {
        onInteractionBlockedChange?.(false);
        onNavigationBlockedChange?.(false);
      },
      [onInteractionBlockedChange, onNavigationBlockedChange],
    );

    const beginWrite = (allowOpenDialog = false) => {
      if (
        writeLockRef.current ||
        mutationPending ||
        (!allowOpenDialog && dialogOpen)
      ) {
        return false;
      }
      writeLockRef.current = true;
      setWritePending(true);
      return true;
    };

    const endWrite = () => {
      writeLockRef.current = false;
      setWritePending(false);
    };

    const enabledCounts = useMemo(() => {
      const counts: Record<ManagedAppId, number> = {
        claude: 0,
        codex: 0,
        opencode: 0,
      };
      for (const skill of skills ?? []) {
        for (const app of SKILLS_APP_IDS) {
          if (skill.apps[app]) counts[app] += 1;
        }
      }
      return counts;
    }, [skills]);

    const filteredSkills = useMemo(() => {
      const query = searchQuery.trim().toLocaleLowerCase();
      if (!query) return skills ?? [];
      return (skills ?? []).filter((skill) =>
        [skill.name, skill.id, skill.description, skill.directory].some(
          (value) => value?.toLocaleLowerCase().includes(query),
        ),
      );
    }, [searchQuery, skills]);

    const sourceFor = (skill: InstalledSkill): ManagedAppId =>
      SKILLS_APP_IDS.find((app) => skill.apps[app]) ?? currentApp;

    const handleToggleApp = async (
      id: string,
      app: ManagedAppId,
      enabled: boolean,
    ) => {
      if (!beginWrite()) return;
      try {
        const skill = skills?.find((candidate) => candidate.id === id);
        await toggleAppMutation.mutateAsync({
          id,
          app,
          sourceApp: skill ? sourceFor(skill) : currentApp,
          enabled,
        });
      } catch (error) {
        toast.error(t("common.error"), { description: String(error) });
      } finally {
        endWrite();
      }
    };

    const handleToggleAll = async (app: ManagedAppId, enabled: boolean) => {
      if (!skills || !beginWrite()) return;
      const ids = skills
        .filter((skill) => Boolean(skill.apps[app]) !== enabled)
        .map((skill) => skill.id);
      if (ids.length === 0) {
        endWrite();
        return;
      }
      const sourceApps = Object.fromEntries(
        skills.map((skill) => [skill.id, sourceFor(skill)]),
      ) as Record<string, ManagedAppId>;
      try {
        const result = await bulkToggleAppMutation.mutateAsync({
          ids,
          app,
          sourceApps,
          enabled,
        });
        if (result.failed.length > 0) {
          toast.error(
            t("common.bulkToggleFailed", { count: result.failed.length }),
            { description: String(result.failed[0].error) },
          );
        }
      } catch (error) {
        toast.error(t("common.bulkToggleFailed", { count: ids.length }), {
          description: String(error),
        });
      } finally {
        endWrite();
      }
    };

    const handleSyncFromLive = async (skill: InstalledSkill) => {
      if (!beginWrite()) return;
      try {
        await syncFromLiveMutation.mutateAsync({
          id: skill.id,
          sourceApp: currentApp,
        });
        toast.success(t("skills.syncSuccess", { name: skill.name }), {
          closeButton: true,
        });
      } catch (error) {
        toast.error(t("skills.syncFailed"), { description: String(error) });
      } finally {
        endWrite();
      }
    };

    const handleDeleteConfirmed = async () => {
      if (!skillToDelete || !beginWrite(true)) return;
      const skill = skillToDelete;
      try {
        const removed = await uninstallMutation.mutateAsync(skill.id);
        setSkillToDelete(null);
        if (removed) {
          toast.success(t("skills.uninstallSuccess", { name: skill.name }), {
            closeButton: true,
          });
        } else {
          toast.error(t("skills.uninstallFailed"));
        }
      } catch (error) {
        toast.error(t("skills.uninstallFailed"), {
          description: String(error),
        });
      } finally {
        endWrite();
      }
    };

    const handleOpenImport = async () => {
      if (!beginWrite()) return;
      setImportDialogOpen(true);
      setScanPending(true);
      try {
        const result = await scanUnmanaged();
        if (result.error) throw result.error;
        if (!result.data || result.data.length === 0) {
          setImportDialogOpen(false);
          toast.success(t("skills.noUnmanagedFound"), { closeButton: true });
          return;
        }
      } catch (error) {
        setImportDialogOpen(false);
        toast.error(t("skills.scanFailed"), { description: String(error) });
      } finally {
        setScanPending(false);
        endWrite();
      }
    };

    const handleImport = async (imports: ImportSkillSelection[]) => {
      if (!beginWrite(true)) return;
      try {
        const imported = await importMutation.mutateAsync(imports);
        setImportDialogOpen(false);
        toast.success(t("skills.importSuccess", { count: imported.length }), {
          closeButton: true,
        });
      } catch (error) {
        toast.error(t("common.error"), { description: String(error) });
      } finally {
        endWrite();
      }
    };

    React.useImperativeHandle(ref, () => ({ openImport: handleOpenImport }));

    const pendingApp: ManagedAppId | null = bulkToggleAppMutation.isPending
      ? (bulkToggleAppMutation.variables?.app ?? null)
      : toggleAppMutation.isPending
        ? (toggleAppMutation.variables?.app ?? null)
        : null;

    return (
      <TooltipProvider delayDuration={300}>
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden px-6">
          <AppCountBar
            totalLabel={t("skills.installed", { count: skills?.length ?? 0 })}
            counts={enabledCounts}
            appIds={SKILLS_APP_IDS}
            totalCount={skills?.length ?? 0}
            onToggleAll={handleToggleAll}
            pendingApp={pendingApp}
            disabled={interactionBlocked}
          />

          <ManagementListSearch
            value={searchQuery}
            onValueChange={setSearchQuery}
            placeholder={t("skills.installedSearchPlaceholder")}
            ariaLabel={t("skills.installedSearchAriaLabel")}
            clearLabel={t("common.clear")}
          />

          <ScrollArea className="-mr-3 min-h-0 flex-1" type="auto">
            <div className="pb-24 pr-3">
              {isLoading ? (
                <div className="py-12 text-center text-muted-foreground">
                  {t("skills.loading")}
                </div>
              ) : installedIsError ? (
                <div className="flex flex-col items-center py-12 text-center">
                  <p className="text-sm font-medium text-foreground">
                    {t("skills.loadFailed")}
                  </p>
                  <p className="mt-1 max-w-lg text-xs text-muted-foreground">
                    {String(installedError)}
                  </p>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="mt-4"
                    onClick={() => void refetchInstalled()}
                  >
                    {t("common.retry", { defaultValue: "重试" })}
                  </Button>
                </div>
              ) : !skills || skills.length === 0 ? (
                <div className="py-12 text-center">
                  <div className="mx-auto mb-4 flex h-16 w-16 items-center justify-center rounded-full bg-muted">
                    <Sparkles size={24} className="text-muted-foreground" />
                  </div>
                  <h3 className="mb-2 text-lg font-medium text-foreground">
                    {t("skills.noInstalled")}
                  </h3>
                  <p className="text-sm text-muted-foreground">
                    {t("skills.noInstalledDescription")}
                  </p>
                  <Button
                    type="button"
                    size="sm"
                    className="mt-4"
                    onClick={() => void handleOpenImport()}
                    disabled={interactionBlocked}
                  >
                    <FolderInput className="mr-2 h-4 w-4" />
                    {t("skills.import")}
                  </Button>
                </div>
              ) : filteredSkills.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-12 text-center text-muted-foreground">
                  <Search className="mb-4 h-10 w-10 opacity-40" />
                  <p className="text-sm">
                    {t("skills.noInstalledSearchResults")}
                  </p>
                </div>
              ) : (
                <div className="overflow-hidden rounded-lg border border-border-default">
                  {filteredSkills.map((skill, index) => (
                    <InstalledSkillListItem
                      key={skill.id}
                      skill={skill}
                      currentApp={currentApp}
                      actionsDisabled={interactionBlocked}
                      isSyncing={
                        syncFromLiveMutation.isPending &&
                        syncFromLiveMutation.variables?.id === skill.id
                      }
                      onToggleApp={handleToggleApp}
                      onSync={() => handleSyncFromLive(skill)}
                      onDelete={() => setSkillToDelete(skill)}
                      onOpenDetail={() => setDetailSkill(skill)}
                      isLast={index === filteredSkills.length - 1}
                    />
                  ))}
                </div>
              )}
            </div>
          </ScrollArea>

          {skillToDelete && (
            <ConfirmDialog
              isOpen={true}
              title={t("skills.uninstall")}
              message={[
                t("skills.uninstallConfirm", { name: skillToDelete.name }),
                t("skills.uninstallAffectList"),
                ...uninstallTargets(skillToDelete),
              ].join("\n")}
              confirmText={t("skills.uninstall")}
              variant="destructive"
              zIndex="top"
              pending={writePending || uninstallMutation.isPending}
              onConfirm={handleDeleteConfirmed}
              onCancel={() => setSkillToDelete(null)}
            />
          )}

          <SkillDetailDialog
            skill={detailSkill}
            onClose={() => setDetailSkill(null)}
          />

          <ImportSkillsDialog
            open={importDialogOpen}
            skills={unmanagedSkills ?? []}
            isScanning={scanPending}
            isImporting={importMutation.isPending}
            onImport={handleImport}
            onClose={() => setImportDialogOpen(false)}
          />
        </div>
      </TooltipProvider>
    );
  },
);

UnifiedSkillsPanel.displayName = "UnifiedSkillsPanel";

interface InstalledSkillListItemProps {
  skill: InstalledSkill;
  currentApp: ManagedAppId;
  actionsDisabled: boolean;
  isSyncing: boolean;
  onToggleApp: (id: string, app: ManagedAppId, enabled: boolean) => void;
  onSync: () => void;
  onDelete: () => void;
  onOpenDetail: () => void;
  isLast: boolean;
}

function InstalledSkillListItem({
  skill,
  currentApp,
  actionsDisabled,
  isSyncing,
  onToggleApp,
  onSync,
  onDelete,
  onOpenDetail,
  isLast,
}: InstalledSkillListItemProps) {
  const { t } = useTranslation();

  return (
    <ListItemRow isLast={isLast}>
      <div className="min-w-0 flex-1">
        <div className="flex min-w-0 items-center gap-2">
          <button
            type="button"
            onClick={onOpenDetail}
            className="truncate text-left text-sm font-medium text-foreground hover:underline"
            title={t("skills.detailOpen")}
          >
            {skill.name}
          </button>
          {!skill.cloudEligible && (
            <Badge
              variant="outline"
              className="h-5 shrink-0 gap-1 px-1.5 text-[10px] text-muted-foreground"
              title={t("skills.localOnlyDescription")}
            >
              <CloudOff size={11} />
              {t("skills.localOnly")}
            </Badge>
          )}
        </div>
        <p
          className="truncate text-xs text-muted-foreground"
          title={skill.directory}
        >
          {skill.directory}
          <span className="text-muted-foreground/60">
            {` · ${formatSkillSize(skill.totalSizeBytes)} · ${t("skills.metaFiles", { count: skill.fileCount })} · ${t("skills.metaUpdated", { date: new Date(skill.updatedAtMs).toLocaleDateString() })}`}
          </span>
        </p>
        <p
          className="min-h-4 truncate text-xs text-muted-foreground/80"
          title={skill.description}
        >
          {skill.description ?? "\u00A0"}
        </p>
      </div>

      <AppToggleGroup
        apps={skill.apps}
        onToggle={(app, enabled) => onToggleApp(skill.id, app, enabled)}
        appIds={SKILLS_APP_IDS}
        disabled={actionsDisabled}
      />

      <div className="flex w-[58px] shrink-0 items-center justify-end gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
        {skill.apps[currentApp] && (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:bg-blue-100 hover:text-blue-500 disabled:opacity-100 dark:hover:bg-blue-500/10 dark:hover:text-blue-400"
            onClick={onSync}
            disabled={actionsDisabled || isSyncing}
            title={t("skills.syncFromLive")}
            aria-label={t("skills.syncFromLive")}
          >
            {isSyncing ? (
              <Loader2 size={14} className="animate-spin" />
            ) : (
              <Copy size={14} />
            )}
          </Button>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 hover:bg-red-100 hover:text-red-500 disabled:opacity-100 dark:hover:bg-red-500/10 dark:hover:text-red-400"
          onClick={onDelete}
          disabled={actionsDisabled}
          title={t("skills.uninstall")}
          aria-label={t("skills.uninstall")}
        >
          <Trash2 size={14} />
        </Button>
      </div>
    </ListItemRow>
  );
}

interface SkillDetailDialogProps {
  skill: InstalledSkill | null;
  onClose: () => void;
}

/** Read-only detail view inspired by SkillManage: metadata, per-client status, live paths. */
function SkillDetailDialog({ skill, onClose }: SkillDetailDialogProps) {
  const { t } = useTranslation();
  const [document, setDocument] = useState<SkillDocumentRead | null>(null);
  const [docLoading, setDocLoading] = useState(false);
  const [docError, setDocError] = useState<string | null>(null);

  React.useEffect(() => {
    setDocument(null);
    setDocError(null);
    setDocLoading(false);
  }, [skill?.id]);

  if (!skill) return null;

  const loadDocument = async () => {
    if (docLoading) return;
    setDocLoading(true);
    setDocError(null);
    try {
      setDocument(await skillsApi.readDocument(skill.id));
    } catch (error) {
      setDocError(String(error));
    } finally {
      setDocLoading(false);
    }
  };

  const openDirectory = async (app: ManagedAppId) => {
    try {
      await skillsApi.openDirectory(skill.id, app);
    } catch (error) {
      toast.error(t("skills.detailOpenDirectory"), {
        description: String(error),
      });
    }
  };

  return (
    <Dialog open={true} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="max-w-2xl" zIndex="alert">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <span className="truncate">{skill.name}</span>
            {!skill.cloudEligible && (
              <Badge
                variant="outline"
                className="h-5 shrink-0 gap-1 px-1.5 text-[10px] text-muted-foreground"
                title={t("skills.localOnlyDescription")}
              >
                <CloudOff size={11} />
                {t("skills.localOnly")}
              </Badge>
            )}
          </DialogTitle>
          <DialogDescription className="line-clamp-2">
            {skill.description || t("skills.detailNoDescription")}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 px-6">
          <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
            <dt className="text-muted-foreground">
              {t("skills.detailDirectory")}
            </dt>
            <dd className="truncate font-mono" title={skill.directory}>
              {skill.directory}
            </dd>
            <dt className="text-muted-foreground">{t("skills.detailSize")}</dt>
            <dd>
              {`${formatSkillSize(skill.totalSizeBytes)} · ${t("skills.metaFiles", { count: skill.fileCount })}`}
            </dd>
            <dt className="text-muted-foreground">{t("skills.detailHash")}</dt>
            <dd className="truncate font-mono" title={skill.contentHash}>
              {skill.contentHash ?? "-"}
            </dd>
            <dt className="text-muted-foreground">
              {t("skills.detailUpdated")}
            </dt>
            <dd>{new Date(skill.updatedAtMs).toLocaleString()}</dd>
          </dl>

          <section>
            <h4 className="mb-2 text-sm font-medium">
              {t("skills.detailAgentStatus")}
            </h4>
            <div className="space-y-1.5">
              {SKILLS_APP_IDS.map((app) => {
                const enabled = skill.apps[app];
                return (
                  <div
                    key={app}
                    className="flex min-w-0 items-center gap-2 rounded-md border border-border-default px-3 py-1.5"
                  >
                    <span className="flex h-6 w-6 shrink-0 items-center justify-center">
                      {APP_ICON_MAP[app].icon}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="text-xs font-medium">
                          {APP_ICON_MAP[app].label}
                        </span>
                        <span
                          className={
                            enabled
                              ? "text-[10px] text-emerald-600 dark:text-emerald-400"
                              : "text-[10px] text-muted-foreground"
                          }
                        >
                          {enabled
                            ? t("skills.detailAgentEnabled")
                            : t("skills.detailAgentDisabled")}
                        </span>
                      </div>
                      <code className="block truncate text-[11px] text-muted-foreground">
                        {enabled
                          ? `${SKILL_LIVE_DIRS[app]}/${skill.directory}`
                          : "-"}
                      </code>
                    </div>
                    {enabled && (
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="h-7 shrink-0 px-2 text-xs"
                        onClick={() => void openDirectory(app)}
                      >
                        <FolderOpen className="mr-1 h-3.5 w-3.5" />
                        {t("skills.detailOpenDirectory")}
                      </Button>
                    )}
                  </div>
                );
              })}
            </div>
          </section>

          <section>
            <div className="mb-2 flex items-center justify-between">
              <h4 className="text-sm font-medium">
                {t("skills.detailPreviewTitle")}
              </h4>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 px-2 text-xs"
                onClick={() => void loadDocument()}
                disabled={docLoading}
              >
                {docLoading ? (
                  <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                ) : (
                  <FileText className="mr-1 h-3.5 w-3.5" />
                )}
                {t("skills.detailPreviewAction")}
              </Button>
            </div>
            {docError && (
              <p className="text-xs text-destructive">
                {`${t("skills.detailPreviewFailed")}: ${docError}`}
              </p>
            )}
            {!docError && !document && !docLoading && (
              <p className="text-xs text-muted-foreground">
                {t("skills.detailPreviewEmpty")}
              </p>
            )}
            {document && (
              <div className="rounded-md border border-border-default">
                <div className="flex items-center gap-2 border-b border-border-default px-3 py-1.5 text-[11px] text-muted-foreground">
                  <span>{t("skills.detailSourceClient")}</span>
                  <span className="font-medium text-foreground">
                    {APP_ICON_MAP[document.sourceClient].label}
                  </span>
                  <span className="ml-auto font-mono">
                    {document.sizeBytes.toLocaleString()} bytes
                  </span>
                </div>
                <pre className="max-h-56 overflow-auto whitespace-pre-wrap break-words px-3 py-2 text-xs leading-5">
                  {document.content}
                </pre>
              </div>
            )}
          </section>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            {t("common.close")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface ImportSkillsDialogProps {
  open: boolean;
  skills: UnmanagedSkill[];
  isScanning: boolean;
  isImporting: boolean;
  onImport: (imports: ImportSkillSelection[]) => void;
  onClose: () => void;
}

function ImportSkillsDialog({
  open,
  skills,
  isScanning,
  isImporting,
  onImport,
  onClose,
}: ImportSkillsDialogProps) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [selectedApps, setSelectedApps] = useState<
    Record<string, ImportSkillSelection["apps"]>
  >({});
  const [selectedSources, setSelectedSources] = useState<
    Record<string, ManagedAppId>
  >({});

  React.useEffect(() => {
    if (!open) return;
    setSelected(new Set(skills.map((skill) => skill.directory)));
    setSelectedApps(
      Object.fromEntries(
        skills.map((skill) => {
          const source = skill.foundIn[0] ?? "claude";
          return [skill.directory, defaultAppsFor(skill, source)];
        }),
      ),
    );
    setSelectedSources(
      Object.fromEntries(
        skills.map((skill) => [skill.directory, skill.foundIn[0] ?? "claude"]),
      ),
    );
  }, [open, skills]);

  const toggleSelected = (directory: string) => {
    setSelected((previous) => {
      const next = new Set(previous);
      if (next.has(directory)) next.delete(directory);
      else next.add(directory);
      return next;
    });
  };

  const submit = () => {
    onImport(
      Array.from(selected).map((directory) => ({
        directory,
        sourceClient: selectedSources[directory] ?? "claude",
        apps: selectedApps[directory] ?? emptyApps(),
      })),
    );
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !isScanning && !isImporting && onClose()}
    >
      <DialogContent className="max-w-2xl" zIndex="alert">
        <DialogHeader>
          <DialogTitle>{t("skills.import")}</DialogTitle>
          <DialogDescription>{t("skills.importDescription")}</DialogDescription>
        </DialogHeader>

        <div className="max-h-[55vh] min-h-32 space-y-2 overflow-y-auto px-6 py-4">
          {isScanning ? (
            <div className="flex min-h-28 items-center justify-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
              {t("skills.scanLoading")}
            </div>
          ) : (
            skills.map((skill) => {
              const source =
                selectedSources[skill.directory] ?? skill.foundIn[0];
              return (
                <div
                  key={skill.directory}
                  className="flex items-start gap-3 rounded-lg border border-border-default p-3"
                >
                  <input
                    type="checkbox"
                    checked={selected.has(skill.directory)}
                    onChange={() => toggleSelected(skill.directory)}
                    aria-label={skill.name}
                    className="mt-1"
                  />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <span className="truncate font-medium">{skill.name}</span>
                      {copiesConsistent(skill) === true && (
                        <Badge
                          variant="outline"
                          className="h-5 shrink-0 gap-1 px-1.5 text-[10px] border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
                          title={t("skills.importCopiesConsistentTitle")}
                        >
                          {t("skills.importCopiesConsistent")}
                        </Badge>
                      )}
                      {copiesConsistent(skill) === false && (
                        <Badge
                          variant="outline"
                          className="h-5 shrink-0 gap-1 px-1.5 text-[10px] border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400"
                          title={t("skills.importCopiesConflictTitle")}
                        >
                          {t("skills.importCopiesConflict")}
                        </Badge>
                      )}
                    </div>
                    {skill.description && (
                      <p className="line-clamp-1 text-sm text-muted-foreground">
                        {skill.description}
                      </p>
                    )}
                    <div className="mt-2 flex flex-wrap items-center gap-3">
                      <label className="flex items-center gap-2 text-xs text-muted-foreground">
                        <span>{t("skills.sourceClient")}</span>
                        <select
                          aria-label={`${t("skills.sourceClient")}: ${skill.name}`}
                          className="h-7 rounded border border-input bg-background px-2 text-xs text-foreground"
                          value={source}
                          onChange={(event) => {
                            const nextSource = event.target
                              .value as ManagedAppId;
                            setSelectedSources((previous) => ({
                              ...previous,
                              [skill.directory]: nextSource,
                            }));
                            setSelectedApps((previous) => ({
                              ...previous,
                              [skill.directory]: defaultAppsFor(
                                skill,
                                nextSource,
                              ),
                            }));
                          }}
                        >
                          {skill.foundIn.map((app) => (
                            <option key={app} value={app}>
                              {t(`skills.apps.${app}`)}
                            </option>
                          ))}
                        </select>
                      </label>
                      <AppToggleGroup
                        apps={selectedApps[skill.directory] ?? emptyApps()}
                        onToggle={(app, enabled) => {
                          if (!enabled && app === source) return;
                          setSelectedApps((previous) => ({
                            ...previous,
                            [skill.directory]: {
                              ...(previous[skill.directory] ?? emptyApps()),
                              [app]: enabled,
                            },
                          }));
                        }}
                        appIds={SKILLS_APP_IDS}
                        disabled={isImporting}
                      />
                    </div>
                    <p
                      className="mt-1 truncate text-xs text-muted-foreground/60"
                      title={skill.path}
                    >
                      {skill.path}
                    </p>
                  </div>
                </div>
              );
            })
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={onClose}
            disabled={isScanning || isImporting}
          >
            {t("common.cancel")}
          </Button>
          <Button
            onClick={submit}
            disabled={selected.size === 0 || isScanning || isImporting}
          >
            {isImporting && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("skills.importSelected", { count: selected.size })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default UnifiedSkillsPanel;
