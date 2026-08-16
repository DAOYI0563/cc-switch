import type { PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  useBulkToggleSkillApp,
  useSyncSkillFromLive,
  useToggleSkillApp,
  useUninstallSkill,
} from "@/hooks/useSkills";
import type { InstalledSkill } from "@/lib/api/skills";

const mocks = vi.hoisted(() => ({
  toggleApp: vi.fn(),
  syncFromLive: vi.fn(),
  uninstallUnified: vi.fn(),
}));

vi.mock("@/lib/api/skills", () => ({ skillsApi: mocks }));

function createClient() {
  return new QueryClient({
    defaultOptions: {
      mutations: { retry: false },
      queries: { retry: false },
    },
  });
}

function wrapper(client: QueryClient) {
  return function Wrapper({ children }: PropsWithChildren) {
    return (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
  };
}

function makeSkill(id: string): InstalledSkill {
  return {
    id,
    name: id,
    directory: id,
    totalSizeBytes: 1,
    fileCount: 1,
    apps: { claude: true, codex: false, opencode: false },
    cloudEligible: true,
    createdAtMs: 1,
    updatedAtMs: 1,
  };
}

describe("local Skill mutation hooks", () => {
  beforeEach(() => {
    for (const mock of Object.values(mocks)) mock.mockReset();
  });

  it("runs bulk live writes sequentially with each Skill's source", async () => {
    const calls: string[] = [];
    mocks.toggleApp.mockImplementation(async (id: string) => {
      calls.push(`start:${id}`);
      await Promise.resolve();
      calls.push(`end:${id}`);
    });
    const client = createClient();
    const { result } = renderHook(() => useBulkToggleSkillApp(), {
      wrapper: wrapper(client),
    });

    let response!: Awaited<ReturnType<typeof result.current.mutateAsync>>;
    await act(async () => {
      response = await result.current.mutateAsync({
        ids: ["alpha", "beta"],
        app: "claude",
        sourceApps: { alpha: "codex", beta: "opencode" },
        enabled: true,
      });
    });

    expect(calls).toEqual([
      "start:alpha",
      "end:alpha",
      "start:beta",
      "end:beta",
    ]);
    expect(mocks.toggleApp).toHaveBeenNthCalledWith(
      1,
      "alpha",
      "claude",
      "codex",
      true,
    );
    expect(mocks.toggleApp).toHaveBeenNthCalledWith(
      2,
      "beta",
      "claude",
      "opencode",
      true,
    );
    expect(response).toEqual({ succeeded: ["alpha", "beta"], failed: [] });
  });

  it("keeps processing local Skills after one bulk write fails", async () => {
    mocks.toggleApp
      .mockRejectedValueOnce(new Error("conflict"))
      .mockResolvedValueOnce(undefined);
    const client = createClient();
    const { result } = renderHook(() => useBulkToggleSkillApp(), {
      wrapper: wrapper(client),
    });

    const response = await act(async () =>
      result.current.mutateAsync({
        ids: ["alpha", "beta"],
        app: "codex",
        sourceApps: { alpha: "claude", beta: "claude" },
        enabled: true,
      }),
    );

    expect(response.succeeded).toEqual(["beta"]);
    expect(response.failed).toEqual([
      { item: "alpha", error: expect.any(Error) },
    ]);
  });

  it("passes an explicit source through a single toggle", async () => {
    mocks.toggleApp.mockResolvedValueOnce(makeSkill("alpha"));
    const client = createClient();
    const { result } = renderHook(() => useToggleSkillApp(), {
      wrapper: wrapper(client),
    });

    await act(async () => {
      await result.current.mutateAsync({
        id: "alpha",
        app: "opencode",
        sourceApp: "codex",
        enabled: true,
      });
    });

    expect(mocks.toggleApp).toHaveBeenCalledWith(
      "alpha",
      "opencode",
      "codex",
      true,
    );
  });

  it("syncs from one live client and refreshes managed and unmanaged lists", async () => {
    mocks.syncFromLive.mockResolvedValueOnce(makeSkill("alpha"));
    const client = createClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const { result } = renderHook(() => useSyncSkillFromLive(), {
      wrapper: wrapper(client),
    });

    await act(async () => {
      await result.current.mutateAsync({ id: "alpha", sourceApp: "opencode" });
    });

    expect(mocks.syncFromLive).toHaveBeenCalledWith("alpha", "opencode");
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "installed"],
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "unmanaged"],
    });
  });

  it("removes only a confirmed deletion from the installed cache", async () => {
    mocks.uninstallUnified.mockResolvedValueOnce(true);
    const client = createClient();
    client.setQueryData(
      ["skills", "installed"],
      [makeSkill("alpha"), makeSkill("beta")],
    );
    const { result } = renderHook(() => useUninstallSkill(), {
      wrapper: wrapper(client),
    });

    await act(async () => {
      await result.current.mutateAsync("alpha");
    });

    expect(
      client.getQueryData<InstalledSkill[]>(["skills", "installed"]),
    ).toEqual([makeSkill("beta")]);
  });
});
