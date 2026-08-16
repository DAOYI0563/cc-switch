import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { Copy, ExternalLink, RefreshCw, Terminal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";

import appIcon from "@/assets/icons/app-icon.png";
import { ProviderIcon } from "@/components/ProviderIcon";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cliStatusApi, settingsApi } from "@/lib/api";
import { copyText } from "@/lib/clipboard";
import type { CliStatus } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

interface AboutSectionProps {
  isPortable: boolean;
}

export function AboutSection({ isPortable }: AboutSectionProps) {
  const { t } = useTranslation();
  const [version, setVersion] = useState<string>();
  const [statuses, setStatuses] = useState<CliStatus[]>([]);
  const [loading, setLoading] = useState(true);

  const load = async () => {
    setLoading(true);
    try {
      const [appVersion, cliStatuses] = await Promise.all([
        getVersion(),
        cliStatusApi.getAll(),
      ]);
      setVersion(appVersion);
      setStatuses(cliStatuses);
    } catch (error) {
      toast.error(extractErrorMessage(error));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load();
  }, []);

  const copyCommand = async (command: string) => {
    try {
      await copyText(command);
      toast.success(
        t("settings.cliStatus.commandCopied", { defaultValue: "命令已复制" }),
      );
    } catch (error) {
      toast.error(extractErrorMessage(error));
    }
  };

  return (
    <TooltipProvider>
      <section className="space-y-6">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border-default pb-5">
          <div className="flex min-w-0 items-center gap-3">
            <img src={appIcon} alt="WSL Code Switch" className="size-10" />
            <div className="min-w-0">
              <h3 className="truncate text-base font-semibold">
                WSL Code Switch
              </h3>
              <div className="mt-1 flex items-center gap-2">
                <Badge variant="outline">v{version ?? "..."}</Badge>
                {isPortable && (
                  <Badge variant="secondary">
                    {t("settings.portableMode", { defaultValue: "便携模式" })}
                  </Badge>
                )}
              </div>
            </div>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void load()}
            disabled={loading}
          >
            <RefreshCw className={`size-4 ${loading ? "animate-spin" : ""}`} />
            {t("common.refresh", { defaultValue: "刷新" })}
          </Button>
        </div>

        <div>
          <div className="mb-3 flex items-center gap-2">
            <Terminal className="size-4 text-muted-foreground" />
            <h4 className="text-sm font-semibold">
              {t("settings.cliStatus.title", { defaultValue: "CLI 状态" })}
            </h4>
          </div>
          <div className="grid grid-cols-1 gap-3 xl:grid-cols-3">
            {statuses.map((status) => (
              <article
                key={status.id}
                className="min-w-0 rounded-md border border-border-default p-4"
              >
                <div className="flex items-center gap-2">
                  <ProviderIcon
                    icon={status.id}
                    name={status.displayName}
                    size={20}
                  />
                  <h5 className="min-w-0 flex-1 truncate text-sm font-semibold">
                    {status.displayName}
                  </h5>
                  <Badge
                    variant={status.state === "ok" ? "secondary" : "outline"}
                  >
                    {status.installationChannel}
                  </Badge>
                </div>

                <dl className="mt-4 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 text-xs">
                  <dt className="text-muted-foreground">
                    {t("settings.cliStatus.current", {
                      defaultValue: "当前版本",
                    })}
                  </dt>
                  <dd className="truncate font-mono">
                    {status.currentVersion ?? "-"}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("settings.cliStatus.latest", {
                      defaultValue: "最新版本",
                    })}
                  </dt>
                  <dd className="flex min-w-0 items-center gap-1">
                    <span className="truncate font-mono">
                      {status.latestVersion ?? "-"}
                    </span>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-6 shrink-0"
                          onClick={() =>
                            settingsApi.openExternal(status.latestSourceUrl)
                          }
                          aria-label={t("settings.cliStatus.openSource", {
                            defaultValue: "打开官方来源",
                          })}
                        >
                          <ExternalLink className="size-3.5" />
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        {t("settings.cliStatus.openSource", {
                          defaultValue: "打开官方来源",
                        })}
                      </TooltipContent>
                    </Tooltip>
                  </dd>
                </dl>

                {status.detail && (
                  <p className="mt-3 text-xs leading-5 text-amber-600 dark:text-amber-400">
                    {status.detail}
                  </p>
                )}

                <div className="mt-4 space-y-2">
                  <CommandRow
                    label="WSL"
                    command={status.wslCommand}
                    onCopy={copyCommand}
                  />
                  <CommandRow
                    label="PowerShell"
                    command={status.powershellCommand}
                    onCopy={copyCommand}
                  />
                </div>
              </article>
            ))}
          </div>
        </div>
      </section>
    </TooltipProvider>
  );
}

function CommandRow({
  label,
  command,
  onCopy,
}: {
  label: string;
  command: string;
  onCopy: (command: string) => Promise<void>;
}) {
  return (
    <div className="grid grid-cols-[78px_minmax(0,1fr)_32px] items-center gap-2 rounded-md bg-muted/50 px-2 py-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <code className="truncate text-xs" title={command}>
        {command}
      </code>
      <Button
        variant="ghost"
        size="icon"
        className="size-7"
        onClick={() => void onCopy(command)}
        aria-label={`复制 ${label} 命令`}
      >
        <Copy className="size-3.5" />
      </Button>
    </div>
  );
}
