import { useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  ChevronLeft,
  ChevronRight,
  Copy,
  ExternalLink,
  Folder,
  RefreshCw,
  Search,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { copyText } from "@/lib/clipboard";
import { sessionsApi } from "@/lib/api/sessions";
import { cn } from "@/lib/utils";
import type { SessionSearchRequest } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";
import { SessionMessageItem } from "./SessionMessageItem";
import {
  formatRelativeTime,
  formatSessionTitle,
  getProviderIconName,
  getProviderLabel,
  getSessionKey,
} from "./utils";

const SESSION_PAGE_SIZE = 50;
const EVENT_PAGE_SIZE = 200;
const MANAGED_CLIENTS = ["claude", "codex", "opencode"] as const;
type ProviderFilter = "all" | (typeof MANAGED_CLIENTS)[number];

function dateBoundary(value: string, endOfDay: boolean): number | undefined {
  if (!value) return undefined;
  const date = new Date(`${value}T00:00:00`);
  if (Number.isNaN(date.getTime())) return undefined;
  return date.getTime() + (endOfDay ? 86_400_000 - 1 : 0);
}

export function SessionManagerPage({ appId }: { appId: string }) {
  const { t } = useTranslation();
  const initialProvider = MANAGED_CLIENTS.includes(
    appId as (typeof MANAGED_CLIENTS)[number],
  )
    ? (appId as ProviderFilter)
    : "all";
  const [providerId, setProviderId] = useState<ProviderFilter>(initialProvider);
  const [keywordInput, setKeywordInput] = useState("");
  const [keyword, setKeyword] = useState("");
  const [project, setProject] = useState("");
  const [fromDate, setFromDate] = useState("");
  const [toDate, setToDate] = useState("");
  const [offset, setOffset] = useState(0);
  const [eventOffset, setEventOffset] = useState(0);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  useEffect(() => {
    const timer = window.setTimeout(() => setKeyword(keywordInput.trim()), 250);
    return () => window.clearTimeout(timer);
  }, [keywordInput]);

  useEffect(() => {
    setOffset(0);
  }, [providerId, keyword, project, fromDate, toDate]);

  const searchRequest = useMemo<SessionSearchRequest>(
    () => ({
      providerId,
      keyword: keyword || undefined,
      project: project.trim() || undefined,
      fromMs: dateBoundary(fromDate, false),
      toMs: dateBoundary(toDate, true),
      offset,
      limit: SESSION_PAGE_SIZE,
    }),
    [fromDate, keyword, offset, project, providerId, toDate],
  );

  const sessionsQuery = useQuery({
    queryKey: ["sessions", "search", searchRequest],
    queryFn: () => sessionsApi.search(searchRequest),
    staleTime: 30_000,
  });
  const page = sessionsQuery.data;
  const sessions = page?.items ?? [];

  useEffect(() => {
    if (sessions.length === 0) {
      setSelectedKey(null);
      return;
    }
    if (
      !selectedKey ||
      !sessions.some((item) => getSessionKey(item) === selectedKey)
    ) {
      setSelectedKey(getSessionKey(sessions[0]));
    }
  }, [selectedKey, sessions]);

  const selectedSession =
    sessions.find((item) => getSessionKey(item) === selectedKey) ?? null;

  useEffect(() => setEventOffset(0), [selectedKey]);

  const messagesQuery = useQuery({
    queryKey: [
      "sessionMessages",
      selectedSession?.providerId,
      selectedSession?.sessionId,
      eventOffset,
    ],
    queryFn: () =>
      sessionsApi.getMessages(
        selectedSession!.providerId,
        selectedSession!.sessionId,
        eventOffset,
        EVENT_PAGE_SIZE,
      ),
    enabled: Boolean(selectedSession),
    staleTime: 30_000,
  });

  const copy = async (value: string, message: string) => {
    try {
      await copyText(value);
      toast.success(message);
    } catch (error) {
      toast.error(extractErrorMessage(error));
    }
  };

  const resume = async () => {
    if (!selectedSession) return;
    try {
      await sessionsApi.launchTerminal({
        providerId: selectedSession.providerId,
        sessionId: selectedSession.sessionId,
      });
    } catch (error) {
      toast.error(extractErrorMessage(error));
    }
  };

  return (
    <TooltipProvider>
      <div className="flex h-full min-h-0 flex-col bg-background">
        <div className="border-b border-border-default px-4 py-3">
          <div className="grid grid-cols-1 gap-2 lg:grid-cols-[minmax(180px,1fr)_180px_minmax(160px,0.7fr)_150px_150px_auto]">
            <div className="relative min-w-0">
              <Search className="pointer-events-none absolute left-3 top-2.5 size-4 text-muted-foreground" />
              <Input
                value={keywordInput}
                onChange={(event) => setKeywordInput(event.target.value)}
                placeholder={t("sessionManager.searchPlaceholder", {
                  defaultValue: "搜索标题、项目或会话内容",
                })}
                className="pl-9"
                aria-label={t("sessionManager.search", {
                  defaultValue: "搜索会话",
                })}
              />
            </div>
            <Select
              value={providerId}
              onValueChange={(value) => setProviderId(value as ProviderFilter)}
            >
              <SelectTrigger
                aria-label={t("sessionManager.providerFilter", {
                  defaultValue: "客户端筛选",
                })}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  {t("common.all", { defaultValue: "全部" })}
                </SelectItem>
                {MANAGED_CLIENTS.map((client) => (
                  <SelectItem key={client} value={client}>
                    {getProviderLabel(client, t)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <div className="relative min-w-0">
              <Folder className="pointer-events-none absolute left-3 top-2.5 size-4 text-muted-foreground" />
              <Input
                value={project}
                onChange={(event) => setProject(event.target.value)}
                placeholder={t("sessionManager.projectFilter", {
                  defaultValue: "项目目录",
                })}
                className="pl-9"
                aria-label={t("sessionManager.projectFilter", {
                  defaultValue: "项目目录",
                })}
              />
            </div>
            <Input
              type="date"
              value={fromDate}
              onChange={(event) => setFromDate(event.target.value)}
              aria-label={t("sessionManager.fromDate", {
                defaultValue: "开始日期",
              })}
            />
            <Input
              type="date"
              value={toDate}
              onChange={(event) => setToDate(event.target.value)}
              aria-label={t("sessionManager.toDate", {
                defaultValue: "结束日期",
              })}
            />
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  type="button"
                  variant="outline"
                  size="icon"
                  onClick={() => sessionsQuery.refetch()}
                  aria-label={t("common.refresh", { defaultValue: "刷新" })}
                >
                  <RefreshCw
                    className={cn(
                      "size-4",
                      sessionsQuery.isFetching && "animate-spin",
                    )}
                  />
                </Button>
              </TooltipTrigger>
              <TooltipContent>
                {t("common.refresh", { defaultValue: "刷新" })}
              </TooltipContent>
            </Tooltip>
          </div>
        </div>

        <div className="grid min-h-0 flex-1 grid-cols-1 md:grid-cols-[minmax(260px,340px)_minmax(0,1fr)]">
          <aside className="flex min-h-0 flex-col border-r border-border-default">
            <div className="min-h-0 flex-1 overflow-y-auto p-2">
              {sessionsQuery.isLoading ? (
                <div className="p-4 text-sm text-muted-foreground">
                  {t("common.loading")}
                </div>
              ) : sessionsQuery.isError ? (
                <div className="p-4 text-sm text-destructive">
                  {extractErrorMessage(sessionsQuery.error)}
                </div>
              ) : sessions.length === 0 ? (
                <div className="p-4 text-sm text-muted-foreground">
                  {t("sessionManager.noSessions", {
                    defaultValue: "没有匹配的会话",
                  })}
                </div>
              ) : (
                <div className="space-y-1">
                  {sessions.map((session) => {
                    const key = getSessionKey(session);
                    const active = key === selectedKey;
                    const timestamp = session.lastActiveAt ?? session.createdAt;
                    return (
                      <button
                        type="button"
                        key={key}
                        onClick={() => setSelectedKey(key)}
                        className={cn(
                          "w-full rounded-md border px-3 py-2 text-left transition-colors",
                          active
                            ? "border-blue-500/40 bg-blue-500/10"
                            : "border-transparent hover:bg-muted/60",
                        )}
                      >
                        <div className="flex min-w-0 items-center gap-2">
                          <ProviderIcon
                            icon={getProviderIconName(session.providerId)}
                            name={session.providerId}
                            size={17}
                          />
                          <span className="min-w-0 flex-1 truncate text-sm font-medium">
                            {formatSessionTitle(session)}
                          </span>
                        </div>
                        <div className="mt-1 flex items-center justify-between gap-2 text-xs text-muted-foreground">
                          <span className="truncate">
                            {session.projectDir || t("common.unknown")}
                          </span>
                          <span className="shrink-0">
                            {timestamp
                              ? formatRelativeTime(timestamp, t)
                              : t("common.unknown")}
                          </span>
                        </div>
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
            <div className="flex h-11 items-center justify-between border-t border-border-default px-2">
              <span className="text-xs text-muted-foreground">
                {t("sessionManager.resultCount", {
                  defaultValue: "{{count}} 条",
                  count: page?.total ?? 0,
                })}
              </span>
              <div className="flex gap-1">
                <Button
                  variant="ghost"
                  size="icon"
                  disabled={offset === 0}
                  onClick={() =>
                    setOffset(Math.max(0, offset - SESSION_PAGE_SIZE))
                  }
                  aria-label={t("common.previous", { defaultValue: "上一页" })}
                >
                  <ChevronLeft className="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  disabled={page?.nextOffset == null}
                  onClick={() =>
                    page?.nextOffset != null && setOffset(page.nextOffset)
                  }
                  aria-label={t("common.next", { defaultValue: "下一页" })}
                >
                  <ChevronRight className="size-4" />
                </Button>
              </div>
            </div>
          </aside>

          <main className="flex min-h-0 min-w-0 flex-col">
            {!selectedSession ? (
              <div className="grid flex-1 place-items-center text-sm text-muted-foreground">
                {t("sessionManager.selectSession", {
                  defaultValue: "选择一个会话查看详情",
                })}
              </div>
            ) : (
              <>
                <div className="flex min-h-14 items-center gap-3 border-b border-border-default px-4 py-2">
                  <div className="min-w-0 flex-1">
                    <h2 className="truncate text-sm font-semibold">
                      {formatSessionTitle(selectedSession)}
                    </h2>
                    <p className="truncate text-xs text-muted-foreground">
                      {selectedSession.projectDir || selectedSession.sessionId}
                    </p>
                  </div>
                  {selectedSession.resumeCommand && (
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="outline"
                          size="icon"
                          onClick={() =>
                            copy(
                              selectedSession.resumeCommand!,
                              t("sessionManager.resumeCommandCopied", {
                                defaultValue: "恢复命令已复制",
                              }),
                            )
                          }
                          aria-label={t("sessionManager.copyResume", {
                            defaultValue: "复制恢复命令",
                          })}
                        >
                          <Copy className="size-4" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        {t("sessionManager.copyResume", {
                          defaultValue: "复制恢复命令",
                        })}
                      </TooltipContent>
                    </Tooltip>
                  )}
                  <Button onClick={resume} size="sm">
                    <ExternalLink className="size-4" />
                    {t("sessionManager.resume", { defaultValue: "恢复会话" })}
                  </Button>
                </div>

                <div className="min-h-0 flex-1 overflow-y-auto p-4">
                  {messagesQuery.isLoading ? (
                    <div className="text-sm text-muted-foreground">
                      {t("common.loading")}
                    </div>
                  ) : messagesQuery.isError ? (
                    <div className="text-sm text-destructive">
                      {extractErrorMessage(messagesQuery.error)}
                    </div>
                  ) : (
                    <div className="mx-auto max-w-4xl space-y-3">
                      {(messagesQuery.data?.items ?? []).map((message) => (
                        <SessionMessageItem
                          key={message.sequence}
                          message={message}
                          isActive={false}
                          searchQuery={keyword}
                          onCopy={(content) =>
                            copy(
                              content,
                              t("sessionManager.messageCopied", {
                                defaultValue: "内容已复制",
                              }),
                            )
                          }
                        />
                      ))}
                    </div>
                  )}
                </div>

                {(messagesQuery.data?.total ?? 0) > EVENT_PAGE_SIZE && (
                  <div className="flex h-11 items-center justify-end gap-1 border-t border-border-default px-3">
                    <Button
                      variant="ghost"
                      size="icon"
                      disabled={eventOffset === 0}
                      onClick={() =>
                        setEventOffset(
                          Math.max(0, eventOffset - EVENT_PAGE_SIZE),
                        )
                      }
                      aria-label={t("common.previous", {
                        defaultValue: "上一页",
                      })}
                    >
                      <ChevronLeft className="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      disabled={messagesQuery.data?.nextOffset == null}
                      onClick={() =>
                        messagesQuery.data?.nextOffset != null &&
                        setEventOffset(messagesQuery.data.nextOffset)
                      }
                      aria-label={t("common.next", { defaultValue: "下一页" })}
                    >
                      <ChevronRight className="size-4" />
                    </Button>
                  </div>
                )}
              </>
            )}
          </main>
        </div>
      </div>
    </TooltipProvider>
  );
}
