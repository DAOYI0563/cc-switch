import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  CheckCircle2,
  Loader2,
  RefreshCw,
  ShieldAlert,
} from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  LOCAL_SCAN_DOMAINS,
  localScanApi,
  type ConflictCenterItem,
  type ConflictResolutionAction,
} from "@/lib/api";
import {
  useConflictCenterItemsQuery,
  useResolveConflictCenterItemMutation,
} from "@/lib/query";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface PendingResolution {
  item: ConflictCenterItem;
  action: ConflictResolutionAction;
}

function shortDigest(value?: string): string {
  return value ? `${value.slice(0, 12)}...${value.slice(-8)}` : "-";
}

function DigestCell({ label, value }: { label: string; value?: string }) {
  return (
    <div className="min-w-0">
      <div className="mb-1 text-[11px] font-medium text-muted-foreground">
        {label}
      </div>
      <code
        title={value}
        className="block min-w-0 truncate rounded bg-muted px-2 py-1.5 text-xs text-foreground"
      >
        {shortDigest(value)}
      </code>
    </div>
  );
}

export function ConflictCenterPage() {
  const { t, i18n } = useTranslation();
  const [pendingResolution, setPendingResolution] =
    useState<PendingResolution | null>(null);
  const query = useConflictCenterItemsQuery();
  const resolveMutation = useResolveConflictCenterItemMutation();
  const items = query.data ?? [];

  useEffect(() => {
    let active = true;
    void Promise.all(
      LOCAL_SCAN_DOMAINS.map((domain) => localScanApi.enterPage(domain)),
    )
      .then(() => {
        if (active) void query.refetch();
      })
      .catch((error) => {
        console.error("[conflict-center] Failed to request local scans", error);
      });
    return () => {
      active = false;
    };
  }, [query.refetch]);

  const actionLabel = (
    item: ConflictCenterItem,
    action: ConflictResolutionAction,
  ): string => {
    if (action === "accept_external") {
      return t(
        item.source === "local_scan"
          ? "conflictCenter.actions.acceptWsl"
          : "conflictCenter.actions.acceptRemote",
      );
    }
    return t(`conflictCenter.actions.${action}`);
  };

  const executeResolution = ({ item, action }: PendingResolution) => {
    resolveMutation.mutate(
      { item, action },
      {
        onSuccess: () => {
          setPendingResolution(null);
          toast.success(t("conflictCenter.resolveSuccess"));
        },
        onError: (error) => {
          toast.error(
            t("conflictCenter.resolveFailed", {
              error: extractErrorMessage(error) || t("common.unknown"),
            }),
          );
        },
      },
    );
  };

  const requestResolution = (
    item: ConflictCenterItem,
    action: ConflictResolutionAction,
  ) => {
    const pending = { item, action };
    if (action === "retry") {
      executeResolution(pending);
    } else {
      setPendingResolution(pending);
    }
  };

  const formatModifiedAt = (value?: number) => {
    if (value === undefined) return t("common.notSet");
    return new Intl.DateTimeFormat(i18n.resolvedLanguage || i18n.language, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(value));
  };

  if (query.isLoading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  if (query.isError) {
    return (
      <div className="mx-auto flex h-full w-full max-w-5xl items-center px-6">
        <div className="flex w-full items-start gap-3 rounded-lg border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-700 dark:text-red-300">
          <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="font-medium">{t("conflictCenter.loadFailed")}</div>
            <div className="mt-1 break-words text-xs opacity-80">
              {extractErrorMessage(query.error)}
            </div>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void query.refetch()}
          >
            <RefreshCw className="h-4 w-4" />
            {t("common.retry", { defaultValue: "重试" })}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="mx-auto flex h-full w-full max-w-6xl flex-col px-6 pb-6">
      <div className="flex shrink-0 items-center justify-between border-b py-4">
        <div className="flex items-center gap-3">
          <ShieldAlert className="h-5 w-5 text-amber-600 dark:text-amber-400" />
          <span className="text-sm font-medium">
            {t("conflictCenter.pendingCount", { count: items.length })}
          </span>
        </div>
        <Button
          variant="ghost"
          size="icon"
          onClick={() => void query.refetch()}
          disabled={query.isRefetching}
          title={t("common.refresh")}
          aria-label={t("common.refresh")}
        >
          <RefreshCw
            className={cn("h-4 w-4", query.isRefetching && "animate-spin")}
          />
        </Button>
      </div>

      {items.length === 0 ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
          <CheckCircle2 className="h-10 w-10 text-emerald-500" />
          <p className="text-sm font-medium">{t("conflictCenter.empty")}</p>
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto py-4">
          <div className="divide-y overflow-hidden rounded-lg border bg-background">
            {items.map((item) => {
              const dispositionKey = `conflictCenter.dispositions.${item.disposition.type}.${item.disposition.kind}`;
              return (
                <article key={item.itemId} className="p-4">
                  <div className="flex flex-wrap items-start justify-between gap-3">
                    <div className="min-w-0">
                      <div className="mb-2 flex flex-wrap items-center gap-2">
                        <Badge variant="outline">
                          {t(`conflictCenter.sources.${item.source}`)}
                        </Badge>
                        <Badge variant="secondary">
                          {t(`conflictCenter.domains.${item.domain}`)}
                        </Badge>
                        {item.clientId ? (
                          <Badge variant="outline">
                            {t(`apps.${item.clientId}`)}
                          </Badge>
                        ) : null}
                        <Badge
                          variant={
                            item.disposition.type === "conflict"
                              ? "destructive"
                              : "default"
                          }
                        >
                          {t(dispositionKey)}
                        </Badge>
                      </div>
                      <h2 className="break-words text-sm font-semibold">
                        {item.displayName}
                      </h2>
                      <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                        {item.recordId ? (
                          <span>
                            {t("conflictCenter.recordId")}: {item.recordId}
                          </span>
                        ) : null}
                        <span>
                          {t("conflictCenter.modifiedAt")}:{" "}
                          {formatModifiedAt(item.modifiedAtMs)}
                        </span>
                        {item.failureKind ? (
                          <span>
                            {t(`conflictCenter.failures.${item.failureKind}`)}
                          </span>
                        ) : null}
                      </div>
                    </div>
                  </div>

                  <div className="mt-4 grid gap-2 sm:grid-cols-3">
                    <DigestCell
                      label={t("conflictCenter.baselineDigest")}
                      value={item.baselineDigest}
                    />
                    <DigestCell
                      label={t("conflictCenter.localDigest")}
                      value={item.localDigest}
                    />
                    <DigestCell
                      label={t("conflictCenter.externalDigest")}
                      value={item.externalDigest}
                    />
                  </div>

                  <div className="mt-4 flex min-h-8 flex-wrap justify-end gap-2">
                    {item.actions.map((action) => {
                      const resolvingThisItem =
                        resolveMutation.isPending &&
                        resolveMutation.variables?.item.itemId ===
                          item.itemId &&
                        resolveMutation.variables.action === action;
                      return (
                        <Button
                          key={action}
                          variant={action === "retry" ? "outline" : "secondary"}
                          size="sm"
                          disabled={resolveMutation.isPending}
                          onClick={() => requestResolution(item, action)}
                        >
                          {resolvingThisItem ? (
                            <Loader2 className="h-4 w-4 animate-spin" />
                          ) : action === "retry" ? (
                            <RefreshCw className="h-4 w-4" />
                          ) : null}
                          {actionLabel(item, action)}
                        </Button>
                      );
                    })}
                  </div>
                </article>
              );
            })}
          </div>
        </div>
      )}

      <ConfirmDialog
        isOpen={pendingResolution !== null}
        title={t("conflictCenter.confirmTitle")}
        message={
          pendingResolution
            ? t("conflictCenter.confirmMessage", {
                name: pendingResolution.item.displayName,
                action: actionLabel(
                  pendingResolution.item,
                  pendingResolution.action,
                ),
              })
            : ""
        }
        confirmText={
          pendingResolution
            ? actionLabel(pendingResolution.item, pendingResolution.action)
            : undefined
        }
        pending={resolveMutation.isPending}
        onConfirm={() => {
          if (pendingResolution) executeResolution(pendingResolution);
        }}
        onCancel={() => setPendingResolution(null)}
      />
    </div>
  );
}
