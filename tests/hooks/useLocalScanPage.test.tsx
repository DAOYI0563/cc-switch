import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const enterPage = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return {
    ...actual,
    localScanApi: { enterPage },
  };
});

import { useLocalScanPage } from "@/hooks/useLocalScanPage";

describe("useLocalScanPage", () => {
  beforeEach(() => {
    enterPage.mockClear();
  });

  it("requests only the entered managed domain", () => {
    type Props = { domain: "provider" | "skill" | null };
    const initialProps: Props = { domain: "provider" };
    const { rerender } = renderHook(
      ({ domain }: Props) => useLocalScanPage(domain),
      { initialProps },
    );

    expect(enterPage).toHaveBeenCalledTimes(1);
    expect(enterPage).toHaveBeenLastCalledWith("provider");

    rerender({ domain: "skill" });
    expect(enterPage).toHaveBeenCalledTimes(2);
    expect(enterPage).toHaveBeenLastCalledWith("skill");

    rerender({ domain: null });
    expect(enterPage).toHaveBeenCalledTimes(2);
  });
});
