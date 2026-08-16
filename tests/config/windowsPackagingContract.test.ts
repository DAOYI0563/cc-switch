import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const repoRoot = resolve(import.meta.dirname, "../..");
const readRepoFile = (path: string) =>
  readFileSync(resolve(repoRoot, path), "utf8");

describe("Windows packaging contract", () => {
  it("disables installers and updater artifacts for the portable executable", () => {
    const config = JSON.parse(
      readRepoFile("src-tauri/tauri.conf.json"),
    ) as Record<string, any>;

    expect(config.bundle.active).toBe(false);
    expect(config.bundle.targets).toBeUndefined();
    expect(config.bundle.createUpdaterArtifacts).toBe(false);
    expect(config.bundle.icon).toEqual(["icons/icon.ico"]);
    expect(config.bundle.windows).toBeUndefined();
    expect(config.bundle.macOS).toBeUndefined();
    expect(config.plugins?.updater).toBeUndefined();
    expect(config.plugins?.["deep-link"]).toBeUndefined();
  });

  it("exposes one explicit Windows x64 portable build command", () => {
    const packageJson = JSON.parse(readRepoFile("package.json")) as Record<
      string,
      any
    >;

    expect(packageJson.scripts.build).toBe("pnpm run build:portable");
    expect(packageJson.scripts["build:portable"]).toBe(
      "pnpm tauri build --target x86_64-pc-windows-msvc --no-bundle",
    );
    expect(
      existsSync(resolve(repoRoot, "src-tauri/wix/per-user-main.wxs")),
    ).toBe(false);
  });

  it("runs backend CI only on Windows and publishes only an x64 EXE", () => {
    const ci = readRepoFile(".github/workflows/ci.yml");
    const release = readRepoFile(".github/workflows/release.yml");

    expect(ci).toContain("runs-on: windows-latest");
    expect(ci).not.toContain(
      "os: [ubuntu-22.04, windows-latest, macos-latest]",
    );

    expect(release).toContain("runs-on: windows-2022");
    expect(release).toContain("pnpm run build:portable");
    expect(release).toContain(
      "src-tauri/target/x86_64-pc-windows-msvc/release/wsl-code-switch.exe",
    );
    expect(release).toContain("Windows-x64-portable.exe");
    expect(release).not.toMatch(/macos|ubuntu|linux|aarch64|arm64/i);
    expect(release).not.toMatch(/msi|msix|nsis|appimage|\.deb|\.rpm|\.dmg/i);
    expect(release).not.toMatch(/updater|latest\.json|\.sig/i);
  });

  it("does not retain the updater-oriented R2 release workflow", () => {
    expect(existsSync(resolve(repoRoot, ".github/workflows/sync-r2.yml"))).toBe(
      false,
    );
  });
});
