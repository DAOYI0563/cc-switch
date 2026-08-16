import type { PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  ConflictCenterItem,
  PortableDomain,
} from "@/lib/api/conflict-center";
import {
  conflictCenterKeys,
  useResolveConflictCenterItemMutation,
} from "@/lib/query/conflict-center";

const resolveMock = vi.hoisted(() => vi.fn());

vi.mock("@/lib/api/conflict-center", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/lib/api/conflict-center")>();
  return {
    ...actual,
    conflictCenterApi: {
      ...actual.conflictCenterApi,
      resolve: resolveMock,
    },
  };
});

function createWrapper(queryClient: QueryClient) {
  return function Wrapper({ children }: PropsWithChildren) {
    return (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
  };
}

function makeItem(domain: PortableDomain): ConflictCenterItem {
  return {
    schemaVersion: 1,
    itemId: `local_${domain}_claude_alpha`,
    source: "local_scan",
    domain,
    clientId: "claude",
    recordId: "alpha",
    displayName: "alpha",
    disposition: { type: "difference", kind: "modified" },
    actions: ["accept_external", "keep_local"],
  };
}

describe("useResolveConflictCenterItemMutation", () => {
  beforeEach(() => {
    resolveMock.mockReset();
    resolveMock.mockResolvedValue(undefined);
  });

  it.each([
    ["provider", ["providers", "claude"]],
    ["mcp", ["mcp", "all"]],
    ["prompt", ["prompts", "claude"]],
    ["skill", ["skills"]],
  ] as const)(
    "invalidates the conflict list and related %s cache after resolution",
    async (domain, relatedKey) => {
      const queryClient = new QueryClient({
        defaultOptions: {
          queries: { retry: false },
          mutations: { retry: false },
        },
      });
      const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries");
      const { result } = renderHook(
        () => useResolveConflictCenterItemMutation(),
        { wrapper: createWrapper(queryClient) },
      );
      const item = makeItem(domain);

      await act(async () => {
        await result.current.mutateAsync({ item, action: "keep_local" });
      });

      expect(resolveMock).toHaveBeenCalledWith({
        itemId: item.itemId,
        action: "keep_local",
      });
      expect(invalidateSpy).toHaveBeenCalledWith({
        queryKey: conflictCenterKeys.all,
      });
      expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: relatedKey });
    },
  );
});
