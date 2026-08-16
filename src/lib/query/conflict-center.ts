import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  conflictCenterApi,
  type ConflictCenterItem,
  type ConflictResolutionAction,
} from "@/lib/api/conflict-center";

export const conflictCenterKeys = {
  all: ["conflict-center"] as const,
  items: ["conflict-center", "items"] as const,
};

export interface UseConflictCenterItemsOptions {
  enabled?: boolean;
}

export function useConflictCenterItemsQuery(
  options: UseConflictCenterItemsOptions = {},
) {
  const { enabled = true } = options;
  return useQuery({
    queryKey: conflictCenterKeys.items,
    queryFn: conflictCenterApi.list,
    enabled,
    refetchInterval: enabled ? 5_000 : false,
    refetchIntervalInBackground: false,
  });
}

export interface ResolveConflictInput {
  item: ConflictCenterItem;
  action: ConflictResolutionAction;
}

export function useResolveConflictCenterItemMutation() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({ item, action }: ResolveConflictInput) =>
      conflictCenterApi.resolve({ itemId: item.itemId, action }),
    onSuccess: async (_result, { item }) => {
      const relatedKey = (() => {
        switch (item.domain) {
          case "provider":
            return ["providers", item.clientId] as const;
          case "mcp":
            return ["mcp", "all"] as const;
          case "prompt":
            return ["prompts", item.clientId] as const;
          case "skill":
            return ["skills"] as const;
          default:
            return null;
        }
      })();
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: conflictCenterKeys.all }),
        relatedKey
          ? queryClient.invalidateQueries({ queryKey: relatedKey })
          : Promise.resolve(),
      ]);
    },
  });
}
