import { useCallback, useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  KeyRound,
  Laptop,
  PlugZap,
  RefreshCw,
  Save,
  ShieldAlert,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { settingsApi } from "@/lib/api";
import type {
  SyncDevice,
  SyncFirstSyncPreview,
  WebDavSyncSettings,
} from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

interface WebdavSyncSectionProps {
  config?: WebDavSyncSettings;
}

type ActionState =
  | "idle"
  | "testing"
  | "saving"
  | "previewing"
  | "confirming"
  | "syncing"
  | "loadingDevices"
  | "retiring";

const formFromConfig = (
  config?: WebDavSyncSettings,
): Required<WebDavSyncSettings> => ({
  baseUrl: config?.baseUrl ?? "",
  username: config?.username ?? "",
  password: config?.password ?? "",
  remoteRoot: config?.remoteRoot ?? "cc-switch-sync",
  profile: config?.profile ?? "default",
});

export function WebdavSyncSection({ config }: WebdavSyncSectionProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [form, setForm] = useState(() => formFromConfig(config));
  const [passwordTouched, setPasswordTouched] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [action, setAction] = useState<ActionState>("idle");
  const [syncPassphrase, setSyncPassphrase] = useState("");
  const [deviceName, setDeviceName] = useState("");
  const [firstSyncPreview, setFirstSyncPreview] =
    useState<SyncFirstSyncPreview | null>(null);
  const [devices, setDevices] = useState<SyncDevice[] | null>(null);
  const [retireTarget, setRetireTarget] = useState<SyncDevice | null>(null);

  useEffect(() => {
    setForm(formFromConfig(config));
    setPasswordTouched(false);
    setDirty(false);
  }, [
    config?.baseUrl,
    config?.password,
    config?.profile,
    config?.remoteRoot,
    config?.username,
  ]);

  const updateField = useCallback(
    (field: keyof Required<WebDavSyncSettings>, value: string) => {
      setForm((current) => ({ ...current, [field]: value }));
      if (field === "password") setPasswordTouched(true);
      setDirty(true);
    },
    [],
  );

  const validatedSettings = useCallback((): WebDavSyncSettings | null => {
    const settings = {
      baseUrl: form.baseUrl.trim(),
      username: form.username.trim(),
      password: form.password,
      remoteRoot: form.remoteRoot.trim() || "cc-switch-sync",
      profile: form.profile.trim() || "default",
    };
    if (!settings.baseUrl) {
      toast.error(t("settings.webdavSync.missingUrl"));
      return null;
    }
    if (!settings.username) {
      toast.error(t("settings.webdavSync.missingUsername"));
      return null;
    }
    return settings;
  }, [form, t]);

  const validateManualAction = useCallback(
    (requireName = false) => {
      if (dirty) {
        toast.error(
          t("settings.webdavSync.saveBeforeSync", {
            defaultValue: "请先保存 WebDAV 配置",
          }),
        );
        return false;
      }
      if (!syncPassphrase) {
        toast.error(
          t("settings.webdavSync.missingPassphrase", {
            defaultValue: "请输入同步口令",
          }),
        );
        return false;
      }
      if (requireName && !deviceName.trim()) {
        toast.error(
          t("settings.webdavSync.missingDeviceName", {
            defaultValue: "请输入设备名称",
          }),
        );
        return false;
      }
      return true;
    },
    [deviceName, dirty, syncPassphrase, t],
  );

  const handleTest = useCallback(async () => {
    const settings = validatedSettings();
    if (!settings) return;
    setAction("testing");
    try {
      await settingsApi.webdavTestConnection(settings, !passwordTouched);
      toast.success(t("settings.webdavSync.testSuccess"));
    } catch (error) {
      toast.error(
        t("settings.webdavSync.testFailed", {
          error: extractErrorMessage(error),
        }),
      );
    } finally {
      setAction("idle");
    }
  }, [passwordTouched, t, validatedSettings]);

  const handleSave = useCallback(async () => {
    const settings = validatedSettings();
    if (!settings) return;
    setAction("saving");
    try {
      await settingsApi.webdavSyncSaveSettings(settings, passwordTouched);
      setDirty(false);
      await queryClient.invalidateQueries({ queryKey: ["settings"] });
      toast.success(
        t("settings.webdavSync.saveSuccess", {
          defaultValue: "WebDAV 配置已保存",
        }),
      );
    } catch (error) {
      toast.error(
        t("settings.webdavSync.saveFailed", {
          error: extractErrorMessage(error),
        }),
      );
    } finally {
      setAction("idle");
    }
  }, [passwordTouched, queryClient, t, validatedSettings]);

  const handlePreview = useCallback(async () => {
    if (!validateManualAction(true)) return;
    setAction("previewing");
    try {
      const preview = await settingsApi.webdavSyncPreviewFirst({
        passphrase: syncPassphrase,
        displayName: deviceName.trim(),
      });
      setFirstSyncPreview(preview);
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setAction("idle");
    }
  }, [deviceName, syncPassphrase, validateManualAction]);

  const handleConfirmFirst = useCallback(async () => {
    if (!firstSyncPreview || !validateManualAction(true)) return;
    setAction("confirming");
    try {
      const result = await settingsApi.webdavSyncConfirmFirst({
        passphrase: syncPassphrase,
        displayName: firstSyncPreview.displayName,
        candidateDeviceId: firstSyncPreview.candidateDeviceId,
        observedAtMs: firstSyncPreview.observedAtMs,
        expectedPreviewToken: firstSyncPreview.previewToken,
      });
      setFirstSyncPreview(null);
      setSyncPassphrase("");
      await queryClient.invalidateQueries({ queryKey: ["conflict-center"] });
      toast.success(
        t("settings.webdavSync.syncSuccess", {
          defaultValue: "同步完成，远端代数 {{generation}}",
          generation: result.committedGeneration,
        }),
      );
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setAction("idle");
    }
  }, [firstSyncPreview, queryClient, syncPassphrase, t, validateManualAction]);

  const handleSyncNow = useCallback(async () => {
    if (!validateManualAction()) return;
    setAction("syncing");
    try {
      const result = await settingsApi.webdavSyncNow(syncPassphrase);
      setSyncPassphrase("");
      await queryClient.invalidateQueries({ queryKey: ["conflict-center"] });
      toast.success(
        t("settings.webdavSync.syncSuccess", {
          defaultValue: "同步完成，远端代数 {{generation}}",
          generation: result.committedGeneration,
        }),
      );
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setAction("idle");
    }
  }, [queryClient, syncPassphrase, t, validateManualAction]);

  const handleDevices = useCallback(async () => {
    if (!validateManualAction()) return;
    setAction("loadingDevices");
    try {
      setDevices(await settingsApi.webdavSyncListDevices(syncPassphrase));
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setAction("idle");
    }
  }, [syncPassphrase, validateManualAction]);

  const handleRetire = useCallback(async () => {
    if (!retireTarget) return;
    setAction("retiring");
    try {
      await settingsApi.webdavSyncRetireDevice(
        syncPassphrase,
        retireTarget.deviceId,
      );
      setRetireTarget(null);
      setDevices(await settingsApi.webdavSyncListDevices(syncPassphrase));
      toast.success(
        t("settings.webdavSync.retireSuccess", {
          defaultValue: "设备已退役",
        }),
      );
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setAction("idle");
    }
  }, [retireTarget, syncPassphrase, t]);

  const busy = action !== "idle";

  return (
    <div className="space-y-5">
      <div className="grid gap-4 md:grid-cols-2">
        <div className="space-y-2 md:col-span-2">
          <Label htmlFor="webdav-base-url">
            {t("settings.webdavSync.baseUrl")}
          </Label>
          <Input
            id="webdav-base-url"
            type="url"
            value={form.baseUrl}
            onChange={(event) => updateField("baseUrl", event.target.value)}
            placeholder={t("settings.webdavSync.baseUrlPlaceholder")}
            disabled={busy}
            autoComplete="url"
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="webdav-username">
            {t("settings.webdavSync.username")}
          </Label>
          <Input
            id="webdav-username"
            value={form.username}
            onChange={(event) => updateField("username", event.target.value)}
            disabled={busy}
            autoComplete="username"
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="webdav-password">
            {t("settings.webdavSync.password")}
          </Label>
          <Input
            id="webdav-password"
            type="password"
            value={form.password}
            onChange={(event) => updateField("password", event.target.value)}
            disabled={busy}
            autoComplete="current-password"
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="webdav-remote-root">
            {t("settings.webdavSync.remoteRoot")}
          </Label>
          <Input
            id="webdav-remote-root"
            value={form.remoteRoot}
            onChange={(event) => updateField("remoteRoot", event.target.value)}
            disabled={busy}
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="webdav-profile">
            {t("settings.webdavSync.profile")}
          </Label>
          <Input
            id="webdav-profile"
            value={form.profile}
            onChange={(event) => updateField("profile", event.target.value)}
            disabled={busy}
          />
        </div>
      </div>

      <div className="flex flex-wrap justify-end gap-2 border-t border-border/60 pt-4">
        <Button
          type="button"
          variant="outline"
          onClick={handleTest}
          disabled={busy}
        >
          <PlugZap className="mr-2 h-4 w-4" />
          {action === "testing"
            ? t("settings.webdavSync.testing")
            : t("settings.webdavSync.test")}
        </Button>
        <Button type="button" onClick={handleSave} disabled={busy || !dirty}>
          <Save className="mr-2 h-4 w-4" />
          {action === "saving"
            ? t("settings.webdavSync.saving")
            : t("settings.webdavSync.save")}
        </Button>
      </div>

      <div className="grid gap-4 border-t border-border/60 pt-5 md:grid-cols-2">
        <div className="space-y-2">
          <Label htmlFor="webdav-sync-passphrase">
            {t("settings.webdavSync.syncPassphrase", {
              defaultValue: "同步口令",
            })}
          </Label>
          <div className="relative">
            <KeyRound className="pointer-events-none absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
            <Input
              id="webdav-sync-passphrase"
              type="password"
              className="pl-9"
              value={syncPassphrase}
              onChange={(event) => setSyncPassphrase(event.target.value)}
              disabled={busy}
              autoComplete="off"
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="webdav-device-name">
            {t("settings.webdavSync.deviceName", { defaultValue: "设备名称" })}
          </Label>
          <Input
            id="webdav-device-name"
            value={deviceName}
            onChange={(event) => setDeviceName(event.target.value)}
            disabled={busy}
            maxLength={128}
          />
        </div>
        <div className="flex flex-wrap justify-end gap-2 md:col-span-2">
          <Button
            type="button"
            variant="outline"
            onClick={handlePreview}
            disabled={busy}
          >
            <ShieldAlert className="mr-2 h-4 w-4" />
            {action === "previewing"
              ? t("settings.webdavSync.previewing", {
                  defaultValue: "预览中...",
                })
              : t("settings.webdavSync.firstSyncPreview", {
                  defaultValue: "首次同步预览",
                })}
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={handleDevices}
            disabled={busy}
          >
            <Laptop className="mr-2 h-4 w-4" />
            {t("settings.webdavSync.deviceManagement", {
              defaultValue: "设备管理",
            })}
          </Button>
          <Button type="button" onClick={handleSyncNow} disabled={busy}>
            <RefreshCw
              className={`mr-2 h-4 w-4 ${action === "syncing" ? "animate-spin" : ""}`}
            />
            {action === "syncing"
              ? t("settings.webdavSync.syncing", { defaultValue: "同步中..." })
              : t("settings.webdavSync.syncNow", { defaultValue: "立即同步" })}
          </Button>
        </div>
      </div>

      <Dialog
        open={firstSyncPreview !== null}
        onOpenChange={(open) => !open && setFirstSyncPreview(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t("settings.webdavSync.firstSyncPreview", {
                defaultValue: "首次同步预览",
              })}
            </DialogTitle>
            <DialogDescription>
              {firstSyncPreview?.displayName}
            </DialogDescription>
          </DialogHeader>
          {firstSyncPreview && (
            <div className="grid grid-cols-2 gap-px bg-border p-px sm:grid-cols-4">
              {(
                [
                  ["additions", "新增"],
                  ["modifications", "修改"],
                  ["deletions", "删除"],
                  ["conflicts", "冲突"],
                ] as const
              ).map(([key, label]) => (
                <div key={key} className="bg-background px-4 py-5 text-center">
                  <div className="text-2xl font-semibold tabular-nums">
                    {firstSyncPreview.changes[key]}
                  </div>
                  <div className="mt-1 text-xs text-muted-foreground">
                    {label}
                  </div>
                </div>
              ))}
            </div>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setFirstSyncPreview(null)}
              disabled={busy}
            >
              {t("common.cancel", { defaultValue: "取消" })}
            </Button>
            <Button type="button" onClick={handleConfirmFirst} disabled={busy}>
              {action === "confirming"
                ? t("settings.webdavSync.syncing", {
                    defaultValue: "同步中...",
                  })
                : t("settings.webdavSync.confirmFirstSync", {
                    defaultValue: "确认并注册设备",
                  })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={devices !== null}
        onOpenChange={(open) => {
          if (!open) {
            setDevices(null);
            setRetireTarget(null);
            setSyncPassphrase("");
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              {t("settings.webdavSync.deviceManagement", {
                defaultValue: "设备管理",
              })}
            </DialogTitle>
          </DialogHeader>
          <div className="max-h-[50vh] space-y-2 overflow-y-auto px-6 py-4">
            {devices?.map((device) => (
              <div
                key={device.deviceId}
                className="flex items-center gap-3 rounded-md border border-border px-3 py-3"
              >
                <Laptop className="h-4 w-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">
                    {device.displayName}
                  </div>
                  <div className="truncate text-xs text-muted-foreground">
                    {device.deviceId}
                  </div>
                </div>
                <span className="text-xs text-muted-foreground">
                  {device.status === "active" ? "活动" : "已退役"}
                </span>
                {device.status === "active" && (
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    title={t("settings.webdavSync.retireDevice", {
                      defaultValue: "退役设备",
                    })}
                    onClick={() => setRetireTarget(device)}
                    disabled={busy}
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                )}
              </div>
            ))}
            {retireTarget && (
              <div className="border border-destructive/50 bg-destructive/5 px-4 py-3">
                <div className="text-sm font-medium">
                  {retireTarget.displayName}
                </div>
                <div className="mt-3 flex justify-end gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    onClick={() => setRetireTarget(null)}
                    disabled={busy}
                  >
                    {t("common.cancel", { defaultValue: "取消" })}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="destructive"
                    onClick={handleRetire}
                    disabled={busy}
                  >
                    {action === "retiring" ? "退役中..." : "确认退役"}
                  </Button>
                </div>
              </div>
            )}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setDevices(null)}
              disabled={busy}
            >
              {t("common.close", { defaultValue: "关闭" })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
