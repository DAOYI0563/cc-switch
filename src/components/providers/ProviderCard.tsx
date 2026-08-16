import { useMemo } from "react";
import type {
  DraggableAttributes,
  DraggableSyntheticListeners,
} from "@dnd-kit/core";
import {
  Copy,
  ExternalLink,
  GripVertical,
  Pencil,
  Plus,
  Power,
  SquareTerminal,
  Trash2,
  Unplug,
} from "lucide-react";

import { ProviderIcon } from "@/components/ProviderIcon";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import type { ManagedAppId } from "@/lib/api";
import { cn } from "@/lib/utils";
import type { Provider } from "@/types";
import { extractCodexBaseUrl } from "@/utils/providerConfigUtils";
import { resolveProviderIcon } from "@/utils/providerIcon";

interface DragHandleProps {
  attributes: DraggableAttributes;
  listeners: DraggableSyntheticListeners;
  isDragging: boolean;
}

export interface ProviderCardProps {
  provider: Provider;
  isCurrent: boolean;
  appId: ManagedAppId;
  isInConfig?: boolean;
  onSwitch: (provider: Provider) => void;
  onEdit: (provider: Provider) => void;
  onDelete: (provider: Provider) => void;
  onRemoveFromConfig?: (provider: Provider) => void;
  onDuplicate: (provider: Provider) => void;
  onOpenWebsite: (url: string) => void;
  onOpenTerminal?: (provider: Provider) => void;
  dragHandleProps?: DragHandleProps;
}

function providerAddress(provider: Provider): string | undefined {
  if (provider.notes?.trim()) return provider.notes.trim();
  if (provider.websiteUrl?.trim()) return provider.websiteUrl.trim();
  const config = provider.settingsConfig as Record<string, unknown>;
  const env = config?.env as Record<string, unknown> | undefined;
  const anthropic = env?.ANTHROPIC_BASE_URL;
  if (typeof anthropic === "string" && anthropic.trim())
    return anthropic.trim();
  if (typeof config?.config === "string")
    return extractCodexBaseUrl(config.config);
  return undefined;
}

export function ProviderCard({
  provider,
  isCurrent,
  appId,
  isInConfig = true,
  onSwitch,
  onEdit,
  onDelete,
  onRemoveFromConfig,
  onDuplicate,
  onOpenWebsite,
  onOpenTerminal,
  dragHandleProps,
}: ProviderCardProps) {
  const address = useMemo(() => providerAddress(provider), [provider]);
  const clickableAddress = Boolean(
    address?.startsWith("http://") || address?.startsWith("https://"),
  );
  const actionLabel =
    appId === "opencode"
      ? isInConfig
        ? "已加入"
        : "加入配置"
      : isCurrent
        ? "使用中"
        : "切换";

  return (
    <div
      className={cn(
        "grid min-h-[88px] gap-3 px-3 py-4 md:grid-cols-[minmax(0,1fr)_auto] md:items-center",
        (isCurrent || (appId === "opencode" && isInConfig)) && "bg-blue-500/5",
        dragHandleProps?.isDragging && "bg-muted shadow-lg",
      )}
    >
      <div className="flex min-w-0 items-center gap-3">
        <button
          type="button"
          aria-label="拖动排序"
          className="shrink-0 cursor-grab p-1 text-muted-foreground active:cursor-grabbing"
          {...(dragHandleProps?.attributes ?? {})}
          {...(dragHandleProps?.listeners ?? {})}
        >
          <GripVertical className="h-4 w-4" />
        </button>
        <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md border border-border-default bg-muted/40">
          <ProviderIcon
            icon={resolveProviderIcon(appId, provider.icon, provider.iconColor)}
            name={provider.name}
            color={provider.iconColor}
            size={20}
          />
        </div>
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="truncate text-sm font-semibold">{provider.name}</h3>
            {provider.category === "official" ? (
              <span className="rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
                官方
              </span>
            ) : null}
          </div>
          {address ? (
            <button
              type="button"
              disabled={!clickableAddress}
              onClick={() => clickableAddress && onOpenWebsite(address)}
              className={cn(
                "mt-1 block max-w-full truncate text-left text-xs text-muted-foreground",
                clickableAddress && "hover:text-foreground hover:underline",
              )}
              title={address}
            >
              {address}
            </button>
          ) : (
            <p className="mt-1 text-xs text-muted-foreground">未配置接口地址</p>
          )}
        </div>
      </div>

      <TooltipProvider delayDuration={300}>
        <div className="flex items-center justify-end gap-1">
          <Button
            size="sm"
            variant={
              isCurrent || (appId === "opencode" && isInConfig)
                ? "outline"
                : "default"
            }
            disabled={appId !== "opencode" && isCurrent}
            onClick={() => onSwitch(provider)}
          >
            {appId === "opencode" ? (
              <Plus className="mr-2 h-4 w-4" />
            ) : (
              <Power className="mr-2 h-4 w-4" />
            )}
            {actionLabel}
          </Button>
          {appId === "opencode" && isInConfig && onRemoveFromConfig ? (
            <IconAction
              label="移出 live 配置"
              icon={Unplug}
              onClick={() => onRemoveFromConfig(provider)}
            />
          ) : null}
          <IconAction
            label="编辑"
            icon={Pencil}
            onClick={() => onEdit(provider)}
          />
          <IconAction
            label="复制"
            icon={Copy}
            onClick={() => onDuplicate(provider)}
          />
          {onOpenTerminal ? (
            <IconAction
              label="在终端打开"
              icon={SquareTerminal}
              onClick={() => onOpenTerminal(provider)}
            />
          ) : null}
          {clickableAddress ? (
            <IconAction
              label="打开网站"
              icon={ExternalLink}
              onClick={() => onOpenWebsite(address!)}
            />
          ) : null}
          <IconAction
            label="删除"
            icon={Trash2}
            destructive
            onClick={() => onDelete(provider)}
          />
        </div>
      </TooltipProvider>
    </div>
  );
}

function IconAction({
  label,
  icon: Icon,
  destructive = false,
  onClick,
}: {
  label: string;
  icon: typeof Pencil;
  destructive?: boolean;
  onClick: () => void;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon"
          aria-label={label}
          onClick={onClick}
        >
          <Icon className={cn("h-4 w-4", destructive && "text-destructive")} />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}
