import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  CalendarDays,
  ExternalLink,
  FileClock,
  FolderOpen,
  Loader2,
  Play,
  RefreshCw,
  Search,
  Settings2,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { dailyBriefApi } from "@/lib/api/daily-brief";
import { cn } from "@/lib/utils";
import type {
  DailyBriefRecord,
  DailyBriefSettingsView,
  SaveDailyBriefSettingsRequest,
} from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

const STATUS: Record<
  DailyBriefRecord["status"],
  { label: string; className: string }
> = {
  disabled: { label: "已关闭", className: "bg-muted text-muted-foreground" },
  pending: { label: "待生成", className: "bg-amber-500/15 text-amber-700" },
  waiting_for_stability: {
    label: "等待会话稳定",
    className: "bg-amber-500/15 text-amber-700",
  },
  running: { label: "生成中", className: "bg-blue-500/15 text-blue-700" },
  pending_resume: {
    label: "待续跑",
    className: "bg-amber-500/15 text-amber-700",
  },
  complete: { label: "完整", className: "bg-emerald-500/15 text-emerald-700" },
  failed: { label: "失败", className: "bg-red-500/15 text-red-700" },
  no_sessions: {
    label: "无会话",
    className: "bg-muted text-muted-foreground",
  },
  integrity_invalid: {
    label: "完整性异常",
    className: "bg-red-500/15 text-red-700",
  },
};

function previousBeijingDate(): string {
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: "Asia/Shanghai",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(Date.now() - 86_400_000);
  const value = Object.fromEntries(
    parts.map((part) => [part.type, part.value]),
  );
  return `${value.year}-${value.month}-${value.day}`;
}

function formatTime(value?: number): string {
  if (!value) return "-";
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value));
}

interface SettingsForm {
  apiUrl: string;
  model: string;
  apiKey: string;
  focus: string;
  autoEnabled: boolean;
  confirmPrivacy: boolean;
}

const EMPTY_FORM: SettingsForm = {
  apiUrl: "",
  model: "",
  apiKey: "",
  focus: "",
  autoEnabled: false,
  confirmPrivacy: false,
};

function settingsForm(settings?: DailyBriefSettingsView): SettingsForm {
  if (!settings) return EMPTY_FORM;
  return {
    apiUrl: settings.apiUrl,
    model: settings.model,
    apiKey: "",
    focus: settings.focus,
    autoEnabled: settings.autoEnabled,
    confirmPrivacy: Boolean(settings.privacyConfirmationHash),
  };
}

export function DailyBriefPage() {
  const queryClient = useQueryClient();
  const [date, setDate] = useState(previousBeijingDate);
  const [queryInput, setQueryInput] = useState("");
  const [query, setQuery] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [form, setForm] = useState<SettingsForm>(EMPTY_FORM);
  const [deleteTarget, setDeleteTarget] = useState<DailyBriefRecord | null>(
    null,
  );

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(queryInput.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [queryInput]);

  const settingsQuery = useQuery({
    queryKey: ["daily-brief", "settings"],
    queryFn: dailyBriefApi.getSettings,
  });
  const recordsQuery = useQuery({
    queryKey: ["daily-brief", "records", query],
    queryFn: () => dailyBriefApi.list(query || undefined),
  });

  useEffect(() => {
    if (settingsOpen) setForm(settingsForm(settingsQuery.data));
  }, [settingsOpen, settingsQuery.data]);

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: ["daily-brief"] });
  };

  const generateMutation = useMutation({
    mutationFn: ({
      targetDate,
      regenerate,
    }: {
      targetDate: string;
      regenerate: boolean;
    }) => dailyBriefApi.generate(targetDate, regenerate),
    onSuccess: async (record) => {
      toast.success(
        record.status === "no_sessions"
          ? "当日无有效会话"
          : "每日简报任务已完成",
      );
      await refresh();
    },
    onError: (error) =>
      toast.error(extractErrorMessage(error) || "每日简报生成失败"),
  });

  const deleteMutation = useMutation({
    mutationFn: (record: DailyBriefRecord) =>
      dailyBriefApi.delete(record.date, record.deviceId),
    onSuccess: async () => {
      setDeleteTarget(null);
      toast.success("每日简报已删除");
      await refresh();
    },
    onError: (error) => toast.error(extractErrorMessage(error) || "删除失败"),
  });

  const settingsMutation = useMutation({
    mutationFn: async (request: SaveDailyBriefSettingsRequest) => {
      const desiredAuto = request.autoEnabled;
      const saved = await dailyBriefApi.saveSettings({
        ...request,
        autoEnabled: false,
      });
      if (!desiredAuto) return saved;
      return await dailyBriefApi.saveSettings({
        ...request,
        apiKey: undefined,
        autoEnabled: true,
      });
    },
    onSuccess: async () => {
      setSettingsOpen(false);
      toast.success("每日简报设置已保存");
      await refresh();
    },
    onError: (error) =>
      toast.error(extractErrorMessage(error) || "设置保存失败"),
  });

  const connectionMutation = useMutation({
    mutationFn: async () => {
      await dailyBriefApi.saveSettings({
        apiUrl: form.apiUrl,
        model: form.model,
        focus: form.focus,
        autoEnabled: false,
        apiKey: form.apiKey.trim() || undefined,
        confirmPrivacy: form.confirmPrivacy,
      });
      return await dailyBriefApi.testConnection();
    },
    onSuccess: async (settings) => {
      setForm((value) => ({ ...value, apiKey: "" }));
      queryClient.setQueryData(["daily-brief", "settings"], settings);
      toast.success("AI 连接测试通过");
    },
    onError: (error) =>
      toast.error(extractErrorMessage(error) || "连接测试失败"),
  });

  const records = recordsQuery.data ?? [];
  const configured = Boolean(
    settingsQuery.data?.apiUrl &&
      settingsQuery.data.model &&
      settingsQuery.data.hasApiKey,
  );
  const busy = generateMutation.isPending;
  const canSave = useMemo(
    () =>
      Boolean(form.apiUrl.trim() && form.model.trim() && form.confirmPrivacy),
    [form.apiUrl, form.confirmPrivacy, form.model],
  );

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="border-b border-border-default px-6 py-4">
        <div className="mx-auto flex w-full max-w-6xl flex-wrap items-end gap-3">
          <div className="min-w-[180px]">
            <Label
              htmlFor="brief-date"
              className="mb-2 block text-xs text-muted-foreground"
            >
              生成日期（北京时间）
            </Label>
            <Input
              id="brief-date"
              type="date"
              value={date}
              onChange={(event) => setDate(event.target.value)}
            />
          </div>
          <Button
            disabled={!date || busy || !configured}
            onClick={() =>
              generateMutation.mutate({ targetDate: date, regenerate: false })
            }
          >
            {busy ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Play className="mr-2 h-4 w-4" />
            )}
            立即生成
          </Button>
          <div className="relative min-w-[220px] flex-1">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={queryInput}
              onChange={(event) => setQueryInput(event.target.value)}
              placeholder="搜索日期、设备或本地简报内容"
              className="pl-9"
            />
          </div>
          <Button
            variant="outline"
            size="icon"
            title="刷新"
            onClick={() => void refresh()}
          >
            <RefreshCw
              className={cn(
                "h-4 w-4",
                recordsQuery.isFetching && "animate-spin",
              )}
            />
          </Button>
          <Button
            variant="outline"
            size="icon"
            title="打开简报目录"
            onClick={() =>
              void dailyBriefApi
                .openDirectory()
                .catch((error) => toast.error(extractErrorMessage(error)))
            }
          >
            <FolderOpen className="h-4 w-4" />
          </Button>
          <Button
            variant="outline"
            size="icon"
            title="简报设置"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings2 className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-6 py-5">
        <div className="mx-auto w-full max-w-6xl">
          {!configured && !settingsQuery.isLoading ? (
            <div className="mb-4 flex items-center justify-between gap-4 border-l-2 border-amber-500 bg-amber-500/10 px-4 py-3 text-sm">
              <span>尚未完成独立 AI API、模型和 API Key 配置。</span>
              <Button
                size="sm"
                variant="outline"
                onClick={() => setSettingsOpen(true)}
              >
                配置
              </Button>
            </div>
          ) : null}

          {recordsQuery.isLoading ? (
            <div className="flex h-48 items-center justify-center">
              <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
            </div>
          ) : recordsQuery.isError ? (
            <div className="py-16 text-center text-sm text-destructive">
              无法读取每日简报记录
            </div>
          ) : records.length === 0 ? (
            <div className="flex h-56 flex-col items-center justify-center gap-3 text-muted-foreground">
              <CalendarDays className="h-8 w-8" />
              <p className="text-sm">暂无匹配的每日简报</p>
            </div>
          ) : (
            <div className="divide-y divide-border-default border-y border-border-default">
              {records.map((record) => {
                const status = STATUS[record.status];
                const canOpen = Boolean(
                  record.localPath || record.status === "complete",
                );
                return (
                  <div
                    key={`${record.date}:${record.deviceId}`}
                    className="grid gap-4 py-4 md:grid-cols-[150px_minmax(0,1fr)_auto] md:items-center"
                  >
                    <div>
                      <div className="font-medium tabular-nums">
                        {record.date}
                      </div>
                      <div
                        className="mt-1 truncate text-xs text-muted-foreground"
                        title={record.deviceId}
                      >
                        {record.deviceId.slice(0, 12)}
                      </div>
                    </div>
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge
                          variant="outline"
                          className={cn("border-0", status.className)}
                        >
                          {status.label}
                        </Badge>
                        {record.sourceState === "changed" ? (
                          <Badge variant="outline">源会话已变化</Badge>
                        ) : null}
                        {record.modelName ? (
                          <span className="truncate text-sm">
                            {record.modelName}
                          </span>
                        ) : null}
                      </div>
                      <div className="mt-1 text-xs text-muted-foreground">
                        {record.generatedAtMs
                          ? `生成于 ${formatTime(record.generatedAtMs)}`
                          : `更新于 ${formatTime(record.updatedAtMs)}`}
                      </div>
                    </div>
                    <div className="flex items-center justify-end gap-1">
                      {record.status === "pending_resume" ||
                      record.status === "failed" ? (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() =>
                            generateMutation.mutate({
                              targetDate: record.date,
                              regenerate: false,
                            })
                          }
                        >
                          <FileClock className="mr-2 h-4 w-4" />
                          续跑
                        </Button>
                      ) : null}
                      {record.status === "complete" ||
                      record.status === "integrity_invalid" ? (
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() =>
                            generateMutation.mutate({
                              targetDate: record.date,
                              regenerate: true,
                            })
                          }
                        >
                          <RefreshCw className="mr-2 h-4 w-4" />
                          重新生成
                        </Button>
                      ) : null}
                      <Button
                        variant="ghost"
                        size="icon"
                        title="打开 HTML"
                        disabled={!canOpen}
                        onClick={() =>
                          void dailyBriefApi
                            .open(record.date, record.deviceId)
                            .catch((error) =>
                              toast.error(
                                extractErrorMessage(error) || "打开失败",
                              ),
                            )
                        }
                      >
                        <ExternalLink className="h-4 w-4" />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        title="删除"
                        onClick={() => setDeleteTarget(record)}
                      >
                        <Trash2 className="h-4 w-4 text-destructive" />
                      </Button>
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>每日简报设置</DialogTitle>
            <DialogDescription>
              此配置仅保存在当前 Windows 设备，不随 WebDAV 同步。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 overflow-y-auto px-6 py-5">
            <div className="space-y-2">
              <Label htmlFor="brief-api-url">OpenAI 兼容 API 地址</Label>
              <Input
                id="brief-api-url"
                value={form.apiUrl}
                onChange={(event) =>
                  setForm((value) => ({ ...value, apiUrl: event.target.value }))
                }
                placeholder="https://api.example.com/v1"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="brief-model">模型</Label>
              <Input
                id="brief-model"
                value={form.model}
                onChange={(event) =>
                  setForm((value) => ({ ...value, model: event.target.value }))
                }
                placeholder="model-name"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="brief-api-key">API Key</Label>
              <Input
                id="brief-api-key"
                type="password"
                value={form.apiKey}
                onChange={(event) =>
                  setForm((value) => ({ ...value, apiKey: event.target.value }))
                }
                placeholder={
                  settingsQuery.data?.hasApiKey
                    ? "已保存在 Windows 凭据管理器；留空不修改"
                    : "输入 API Key"
                }
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="brief-focus">关注重点</Label>
              <Textarea
                id="brief-focus"
                value={form.focus}
                maxLength={1000}
                onChange={(event) =>
                  setForm((value) => ({ ...value, focus: event.target.value }))
                }
                placeholder="可选，例如：重点整理完成事项、风险和次日计划"
              />
            </div>
            <label className="flex items-start gap-3 border-l-2 border-amber-500 bg-amber-500/10 px-4 py-3 text-sm leading-relaxed">
              <input
                type="checkbox"
                className="mt-1"
                checked={form.confirmPrivacy}
                onChange={(event) =>
                  setForm((value) => ({
                    ...value,
                    confirmPrivacy: event.target.checked,
                  }))
                }
              />
              <span>
                我确认：脱敏后的会话内容仍会发送到自定义第三方
                API，其数据保留策略由服务商决定；本地完整 HTML 为明文文件。
              </span>
            </label>
            <div className="flex items-center justify-between gap-4 border-t border-border-default pt-4">
              <div>
                <div className="text-sm font-medium">每天 08:00 自动生成</div>
                <div className="text-xs text-muted-foreground">
                  按北京时间处理前一自然日，默认关闭。
                </div>
              </div>
              <Switch
                checked={form.autoEnabled}
                onCheckedChange={(checked) =>
                  setForm((value) => ({ ...value, autoEnabled: checked }))
                }
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setSettingsOpen(false)}>
              取消
            </Button>
            <Button
              variant="outline"
              disabled={!canSave || connectionMutation.isPending}
              onClick={() => connectionMutation.mutate()}
            >
              {connectionMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : null}
              测试连接
            </Button>
            <Button
              disabled={!canSave || settingsMutation.isPending}
              onClick={() =>
                settingsMutation.mutate({
                  apiUrl: form.apiUrl,
                  model: form.model,
                  focus: form.focus,
                  autoEnabled: form.autoEnabled,
                  apiKey: form.apiKey.trim() || undefined,
                  confirmPrivacy: form.confirmPrivacy,
                })
              }
            >
              {settingsMutation.isPending ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : null}
              保存
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        isOpen={deleteTarget !== null}
        title="删除每日简报"
        message={
          deleteTarget
            ? `将删除 ${deleteTarget.date} 的本地 HTML、索引和加密缓存。下次手动同步会传播删除。`
            : ""
        }
        confirmText="删除"
        pending={deleteMutation.isPending}
        onConfirm={() => deleteTarget && deleteMutation.mutate(deleteTarget)}
        onCancel={() => setDeleteTarget(null)}
      />
    </div>
  );
}
