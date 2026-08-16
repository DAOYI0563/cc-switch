import { describe, expect, it } from "vitest";

import {
  MANAGED_APP_IDS,
  isManagedAppId,
  readStoredManagedAppId,
} from "./types";

describe("managed app contract", () => {
  it("registers exactly Claude Code, Codex, and OpenCode", () => {
    expect(MANAGED_APP_IDS).toEqual(["claude", "codex", "opencode"]);
  });

  it.each([
    "claude-desktop",
    "gemini",
    "grokbuild",
    "openclaw",
    "hermes",
    null,
  ])("rejects legacy or invalid app id %s", (value) => {
    expect(isManagedAppId(value)).toBe(false);
  });

  it("falls back from a legacy persisted app to Claude", () => {
    const storage = { getItem: () => "gemini" };
    expect(readStoredManagedAppId(storage, "cc-switch-last-app")).toBe(
      "claude",
    );
  });

  it("restores a managed persisted app", () => {
    const storage = { getItem: () => "opencode" };
    expect(readStoredManagedAppId(storage, "cc-switch-last-app")).toBe(
      "opencode",
    );
  });
});
