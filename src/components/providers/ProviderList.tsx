import { useMemo, useState } from "react";
import { closestCenter, DndContext } from "@dnd-kit/core";
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Search } from "lucide-react";
import { toast } from "sonner";

import { ProviderCard } from "@/components/providers/ProviderCard";
import { ProviderEmptyState } from "@/components/providers/ProviderEmptyState";
import { Input } from "@/components/ui/input";
import { useDragSort } from "@/hooks/useDragSort";
import type { ManagedAppId } from "@/lib/api";
import { providersApi } from "@/lib/api/providers";
import type { Provider } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";

interface ProviderListProps {
  providers: Record<string, Provider>;
  currentProviderId: string;
  appId: ManagedAppId;
  onSwitch: (provider: Provider) => void;
  onEdit: (provider: Provider) => void;
  onDelete: (provider: Provider) => void;
  onRemoveFromConfig?: (provider: Provider) => void;
  onDuplicate: (provider: Provider) => void;
  onOpenWebsite: (url: string) => void;
  onOpenTerminal?: (provider: Provider) => void;
  onCreate?: () => void;
  isLoading?: boolean;
}

export function ProviderList({
  providers,
  currentProviderId,
  appId,
  onSwitch,
  onEdit,
  onDelete,
  onRemoveFromConfig,
  onDuplicate,
  onOpenWebsite,
  onOpenTerminal,
  onCreate,
  isLoading = false,
}: ProviderListProps) {
  const [search, setSearch] = useState("");
  const queryClient = useQueryClient();
  const { sortedProviders, sensors, handleDragEnd } = useDragSort(
    providers,
    appId,
  );
  const liveIds = useQuery({
    queryKey: ["opencodeLiveProviderIds"],
    queryFn: providersApi.getOpenCodeLiveProviderIds,
    enabled: appId === "opencode",
  });
  const importMutation = useMutation({
    mutationFn: async () =>
      appId === "opencode"
        ? (await providersApi.importOpenCodeFromLive()) > 0
        : await providersApi.importDefault(appId),
    onSuccess: async (imported) => {
      await queryClient.invalidateQueries({ queryKey: ["providers", appId] });
      toast[imported ? "success" : "info"](
        imported ? "已导入当前 WSL 配置" : "未发现可导入配置",
      );
    },
    onError: (error) => toast.error(extractErrorMessage(error) || "导入失败"),
  });

  const filtered = useMemo(() => {
    const keyword = search.trim().toLowerCase();
    if (!keyword) return sortedProviders;
    return sortedProviders.filter((provider) =>
      [provider.name, provider.notes, provider.websiteUrl].some((value) =>
        value?.toLowerCase().includes(keyword),
      ),
    );
  }, [search, sortedProviders]);

  if (isLoading) {
    return (
      <div className="h-40 animate-pulse rounded-md border border-dashed bg-muted/30" />
    );
  }
  if (sortedProviders.length === 0) {
    return (
      <ProviderEmptyState
        appId={appId}
        onCreate={onCreate}
        onImport={() => importMutation.mutate()}
      />
    );
  }

  return (
    <div className="mt-4 space-y-3">
      <div className="relative ml-auto max-w-sm">
        <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="搜索名称、备注或地址"
          className="pl-9"
        />
      </div>
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext
          items={filtered.map((provider) => provider.id)}
          strategy={verticalListSortingStrategy}
        >
          <div className="divide-y divide-border-default border-y border-border-default">
            {filtered.map((provider) => (
              <SortableProvider
                key={provider.id}
                provider={provider}
                appId={appId}
                isCurrent={
                  appId !== "opencode" && provider.id === currentProviderId
                }
                isInConfig={
                  appId !== "opencode" ||
                  Boolean(liveIds.data?.includes(provider.id))
                }
                onSwitch={onSwitch}
                onEdit={onEdit}
                onDelete={onDelete}
                onRemoveFromConfig={onRemoveFromConfig}
                onDuplicate={onDuplicate}
                onOpenWebsite={onOpenWebsite}
                onOpenTerminal={onOpenTerminal}
              />
            ))}
          </div>
        </SortableContext>
      </DndContext>
      {filtered.length === 0 ? (
        <p className="py-10 text-center text-sm text-muted-foreground">
          没有匹配的供应商
        </p>
      ) : null}
    </div>
  );
}

type SortableProviderProps = Omit<
  React.ComponentProps<typeof ProviderCard>,
  "dragHandleProps"
>;

function SortableProvider(props: SortableProviderProps) {
  const sortable = useSortable({ id: props.provider.id });
  return (
    <div
      ref={sortable.setNodeRef}
      style={{
        transform: CSS.Transform.toString(sortable.transform),
        transition: sortable.transition,
        opacity: sortable.isDragging ? 0.7 : 1,
      }}
    >
      <ProviderCard
        {...props}
        dragHandleProps={{
          attributes: sortable.attributes,
          listeners: sortable.listeners,
          isDragging: sortable.isDragging,
        }}
      />
    </div>
  );
}
