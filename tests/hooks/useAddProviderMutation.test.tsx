import type { ReactNode } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAddProviderMutation } from "@/lib/query/mutations";

const apiMocks = vi.hoisted(() => ({
  add: vi.fn(),
  updateTrayMenu: vi.fn(),
}));

const uuidMocks = vi.hoisted(() => ({
  generateUUID: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  providersApi: {
    add: (...args: unknown[]) => apiMocks.add(...args),
    updateTrayMenu: (...args: unknown[]) => apiMocks.updateTrayMenu(...args),
  },
  sessionsApi: {},
  settingsApi: {},
}));

vi.mock("@/utils/uuid", () => ({
  generateUUID: () => uuidMocks.generateUUID(),
}));

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}));

function createWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  );

  return { wrapper };
}

beforeEach(() => {
  apiMocks.add.mockReset().mockResolvedValue(true);
  apiMocks.updateTrayMenu.mockReset().mockResolvedValue(true);
  uuidMocks.generateUUID.mockReset().mockReturnValue("generated-uuid");
});

describe("useAddProviderMutation", () => {
  it("creates a Claude provider with a generated id", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("claude"), {
      wrapper,
    });

    const createdProvider = await act(async () =>
      result.current.mutateAsync({
        name: "Claude Custom",
        settingsConfig: { env: {} },
        category: "custom",
      }),
    );

    expect(apiMocks.add).toHaveBeenCalledTimes(1);
    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({
        id: "generated-uuid",
        name: "Claude Custom",
        category: "custom",
      }),
      "claude",
      undefined,
    );
    expect(createdProvider.id).toBe("generated-uuid");
  });

  it("uses the OpenCode provider key as the stable id", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("opencode"), {
      wrapper,
    });

    const createdProvider = await act(async () =>
      result.current.mutateAsync({
        name: "OpenCode Custom",
        settingsConfig: {
          npm: "@ai-sdk/openai-compatible",
          options: {},
          models: {},
        },
        category: "custom",
        providerKey: "custom-provider",
      }),
    );

    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({ id: "custom-provider" }),
      "opencode",
      undefined,
    );
    expect(createdProvider.id).toBe("custom-provider");
    expect(createdProvider).not.toHaveProperty("providerKey");
  });

  it("creates a Codex provider with a generated id", async () => {
    const { wrapper } = createWrapper();
    const { result } = renderHook(() => useAddProviderMutation("codex"), {
      wrapper,
    });

    const createdProvider = await act(async () =>
      result.current.mutateAsync({
        name: "Codex Custom",
        settingsConfig: { auth: {}, config: "" },
        category: "custom",
      }),
    );

    expect(apiMocks.add).toHaveBeenCalledWith(
      expect.objectContaining({ id: "generated-uuid", name: "Codex Custom" }),
      "codex",
      undefined,
    );
    expect(createdProvider.id).toBe("generated-uuid");
  });
});
