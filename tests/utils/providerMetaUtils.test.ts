import { describe, expect, it } from "vitest";
import { normalizeProviderMeta } from "@/utils/providerMetaUtils";

describe("normalizeProviderMeta", () => {
  it("keeps only supported provider metadata", () => {
    expect(
      normalizeProviderMeta({
        custom_endpoints: {
          "https://old.example": {
            url: "https://old.example",
            addedAt: 1,
          },
        },
        endpointAutoSelect: true,
        commonConfigEnabled: true,
        customUserAgent: "wsl-code-switch-test",
      }),
    ).toEqual({
      commonConfigEnabled: true,
      customUserAgent: "wsl-code-switch-test",
    });
  });

  it("returns undefined when no supported metadata remains", () => {
    expect(
      normalizeProviderMeta({
        custom_endpoints: {},
        endpointAutoSelect: false,
      }),
    ).toBeUndefined();
  });
});
