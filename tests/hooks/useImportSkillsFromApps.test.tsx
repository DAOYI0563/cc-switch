import type { PropsWithChildren } from "react";
import { act, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { mergeImportedSkills } from "@/hooks/useSkills.helpers";
import { useImportSkillsFromApps } from "@/hooks/useSkills";
import type { InstalledSkill } from "@/lib/api/skills";

const importFromApps = vi.hoisted(() => vi.fn());

vi.mock("@/lib/api/skills", () => ({
  skillsApi: { importFromApps },
}));

function makeSkill(overrides: Partial<InstalledSkill> = {}): InstalledSkill {
  return {
    id: "skill-a",
    name: "Skill A",
    directory: "skill-a",
    contentHash: "hash-a",
    totalSizeBytes: 32,
    fileCount: 1,
    apps: { claude: true, codex: false, opencode: false },
    cloudEligible: true,
    createdAtMs: 1,
    updatedAtMs: 2,
    ...overrides,
  };
}

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

describe("mergeImportedSkills", () => {
  it("deduplicates existing and incoming records by stable local ID", () => {
    const stale = makeSkill({ name: "Stale" });
    const fresh = makeSkill({ name: "Fresh" });
    const second = makeSkill({ id: "skill-b", name: "Skill B" });

    expect(mergeImportedSkills([stale], [fresh, second])).toEqual([
      fresh,
      second,
    ]);
  });

  it("keeps the existing cache reference for an empty result", () => {
    const existing = [makeSkill()];
    expect(mergeImportedSkills(existing, [])).toBe(existing);
  });

  it("keeps the last duplicate from one scan", () => {
    const first = makeSkill({ name: "First" });
    const last = makeSkill({ name: "Last" });
    expect(mergeImportedSkills(undefined, [first, last])).toEqual([last]);
  });
});

describe("useImportSkillsFromApps", () => {
  beforeEach(() => importFromApps.mockReset());

  it("imports only the explicit local selections and refreshes both lists", async () => {
    const client = createClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    const stale = makeSkill({ name: "Stale" });
    const fresh = makeSkill({ name: "Fresh" });
    client.setQueryData(["skills", "installed"], [stale]);
    importFromApps.mockResolvedValueOnce([fresh]);
    const selection = [
      {
        directory: "skill-a",
        sourceClient: "claude" as const,
        apps: { claude: true, codex: true, opencode: false },
      },
    ];
    const { result } = renderHook(() => useImportSkillsFromApps(), {
      wrapper: wrapper(client),
    });

    await act(async () => {
      await result.current.mutateAsync(selection);
    });

    expect(importFromApps).toHaveBeenCalledWith(selection);
    expect(client.getQueryData(["skills", "installed"])).toEqual([fresh]);
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "installed"],
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "unmanaged"],
    });
  });

  it("refreshes local scans after a rejected import", async () => {
    const client = createClient();
    const invalidate = vi.spyOn(client, "invalidateQueries");
    importFromApps.mockRejectedValueOnce(new Error("invalid tree"));
    const { result } = renderHook(() => useImportSkillsFromApps(), {
      wrapper: wrapper(client),
    });

    await act(async () => {
      await expect(result.current.mutateAsync([])).rejects.toThrow(
        "invalid tree",
      );
    });

    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "installed"],
    });
    expect(invalidate).toHaveBeenCalledWith({
      queryKey: ["skills", "unmanaged"],
    });
  });
});
