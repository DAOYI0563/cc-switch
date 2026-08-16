import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = process.cwd();
const read = (path: string) => readFileSync(resolve(root, path), "utf8");

describe("manual provider network tools contract", () => {
  it("removes connectivity and endpoint speed-test surfaces", () => {
    const settingsPage = read("src/components/settings/SettingsPage.tsx");
    const providerForm = read(
      "src/components/providers/forms/ProviderForm.tsx",
    );

    expect(existsSync(resolve(root, "src/lib/api/connectivity-check.ts"))).toBe(
      false,
    );
    expect(existsSync(resolve(root, "src/lib/api/vscode.ts"))).toBe(false);
    expect(settingsPage).not.toContain("ConnectivityCheckConfigPanel");
    expect(providerForm).toContain("handleFetchModels");
    expect(providerForm).not.toMatch(
      /EndpointSpeedTest|useSpeedTestEndpoints|streamCheck/,
    );
  });
});
