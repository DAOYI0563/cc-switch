import { describe, expect, it } from "vitest";
import { resolveProviderIcon } from "./providerIcon";

describe("resolveProviderIcon", () => {
  it("preserves a selected icon", () => {
    expect(resolveProviderIcon("claude", "anthropic", "")).toBe("anthropic");
  });

  it("does not reinterpret a provider icon", () => {
    expect(resolveProviderIcon("codex", "grok", "")).toBe("grok");
  });

  it("normalizes an empty icon to the initials fallback", () => {
    expect(resolveProviderIcon("opencode", "  ", "")).toBeUndefined();
  });
});
