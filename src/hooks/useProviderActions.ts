import { useCallback } from "react";
import { toast } from "sonner";

import {
  useAddProviderMutation,
  useDeleteProviderMutation,
  useSwitchProviderMutation,
  useUpdateProviderMutation,
} from "@/lib/query";
import { providersApi, type ManagedAppId } from "@/lib/api";
import type { Provider } from "@/types";

export function useProviderActions(activeApp: ManagedAppId) {
  const addMutation = useAddProviderMutation(activeApp);
  const updateMutation = useUpdateProviderMutation(activeApp);
  const deleteMutation = useDeleteProviderMutation(activeApp);
  const switchMutation = useSwitchProviderMutation(activeApp);

  const addProvider = useCallback(
    async (
      provider: Omit<Provider, "id"> & {
        providerKey?: string;
        addToLive?: boolean;
      },
    ) => {
      await addMutation.mutateAsync(provider);
    },
    [addMutation],
  );

  const updateProvider = useCallback(
    async (provider: Provider, originalId?: string) => {
      await updateMutation.mutateAsync({ provider, originalId });
      void providersApi.updateTrayMenu().catch(() => undefined);
    },
    [updateMutation],
  );

  const switchProvider = useCallback(
    async (provider: Provider) => {
      try {
        await switchMutation.mutateAsync(provider.id);
        toast.success(
          activeApp === "opencode" ? "已加入 OpenCode 配置" : "供应商已切换",
        );
      } catch {
        // Mutation owns the detailed error toast.
      }
    },
    [activeApp, switchMutation],
  );

  const deleteProvider = useCallback(
    async (id: string) => deleteMutation.mutateAsync(id),
    [deleteMutation],
  );

  return {
    addProvider,
    updateProvider,
    switchProvider,
    deleteProvider,
    isLoading:
      addMutation.isPending ||
      updateMutation.isPending ||
      deleteMutation.isPending ||
      switchMutation.isPending,
  };
}
