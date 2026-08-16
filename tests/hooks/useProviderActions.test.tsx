import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useProviderActions } from "@/hooks/useProviderActions";
import type { Provider } from "@/types";

const toastSuccess = vi.fn();
const add = vi.fn();
const update = vi.fn();
const remove = vi.fn();
const switchProvider = vi.fn();
const updateTrayMenu = vi.fn();

const mutations = {
  add: { mutateAsync: add, isPending: false },
  update: { mutateAsync: update, isPending: false },
  remove: { mutateAsync: remove, isPending: false },
  switchProvider: { mutateAsync: switchProvider, isPending: false },
};

vi.mock("sonner", () => ({
  toast: { success: (...args: unknown[]) => toastSuccess(...args) },
}));

vi.mock("@/lib/query", () => ({
  useAddProviderMutation: () => mutations.add,
  useUpdateProviderMutation: () => mutations.update,
  useDeleteProviderMutation: () => mutations.remove,
  useSwitchProviderMutation: () => mutations.switchProvider,
}));

vi.mock("@/lib/api", () => ({
  providersApi: { updateTrayMenu: () => updateTrayMenu() },
}));

function wrapper({ children }: { children: ReactNode }) {
  return (
    <QueryClientProvider client={new QueryClient()}>
      {children}
    </QueryClientProvider>
  );
}

function provider(): Provider {
  return {
    id: "provider-1",
    name: "测试供应商",
    settingsConfig: {},
    category: "custom",
  };
}

beforeEach(() => {
  add.mockReset();
  update.mockReset();
  remove.mockReset();
  switchProvider.mockReset();
  updateTrayMenu.mockReset();
  toastSuccess.mockReset();
  Object.values(mutations).forEach((mutation) => {
    mutation.isPending = false;
  });
});

describe("useProviderActions", () => {
  it("delegates provider CRUD to the managed mutations", async () => {
    add.mockResolvedValue(undefined);
    update.mockResolvedValue(undefined);
    remove.mockResolvedValue(undefined);
    updateTrayMenu.mockResolvedValue(true);
    const item = provider();
    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    await act(async () => {
      await result.current.addProvider({
        name: "新增供应商",
        settingsConfig: {},
      });
      await result.current.updateProvider(item, "old-provider");
      await result.current.deleteProvider(item.id);
    });

    expect(add).toHaveBeenCalledWith({
      name: "新增供应商",
      settingsConfig: {},
    });
    expect(update).toHaveBeenCalledWith({
      provider: item,
      originalId: "old-provider",
    });
    expect(remove).toHaveBeenCalledWith(item.id);
    expect(updateTrayMenu).toHaveBeenCalledOnce();
  });

  it("switches through the three-client command surface", async () => {
    switchProvider.mockResolvedValue(undefined);
    const item = provider();
    const { result } = renderHook(() => useProviderActions("opencode"), {
      wrapper,
    });

    await act(async () => {
      await result.current.switchProvider(item);
    });

    expect(switchProvider).toHaveBeenCalledWith(item.id);
    expect(toastSuccess).toHaveBeenCalledWith("已加入 OpenCode 配置");
  });

  it("keeps mutation failures owned by the query layer", async () => {
    switchProvider.mockRejectedValue(new Error("switch failed"));
    const { result } = renderHook(() => useProviderActions("codex"), {
      wrapper,
    });

    await expect(result.current.switchProvider(provider())).resolves.toBeUndefined();
    expect(toastSuccess).not.toHaveBeenCalled();
  });

  it("reports whether any retained mutation is running", () => {
    mutations.update.isPending = true;
    const { result } = renderHook(() => useProviderActions("claude"), {
      wrapper,
    });

    expect(result.current.isLoading).toBe(true);
  });
});
