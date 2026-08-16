import { existsSync, readFileSync, readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = process.cwd();
const read = (relativePath: string) =>
  readFileSync(resolve(root, relativePath), "utf8");

describe("local-only Skill production surface", () => {
  it("removes online discovery, install, update, backup, and storage UIs", () => {
    for (const relativePath of [
      "src/components/skills/SkillsPage.tsx",
      "src/components/skills/RepoManagerPanel.tsx",
      "src/components/skills/SkillCard.tsx",
      "src/components/settings/SkillStorageLocationSettings.tsx",
      "src/components/settings/SkillSyncMethodSettings.tsx",
      "src/components/deeplink/SkillConfirmation.tsx",
      "src/components/DeepLinkImportDialog.tsx",
      "src/lib/api/deeplink.ts",
      "src/lib/errors/skillErrorParser.ts",
      "tests/components/SkillsPageInstall.test.tsx",
    ]) {
      expect(existsSync(resolve(root, relativePath)), relativePath).toBe(false);
    }
  });

  it("keeps only local Skill API and hook capabilities", () => {
    const api = read("src/lib/api/skills.ts");
    const hooks = read("src/hooks/useSkills.ts");
    const app = read("src/App.tsx");
    const panel = read("src/components/skills/UnifiedSkillsPanel.tsx");

    for (const required of [
      "getInstalled",
      "uninstallUnified",
      "toggleApp",
      "syncFromLive",
      "scanUnmanaged",
      "importFromApps",
    ]) {
      expect(api, required).toContain(required);
    }

    const productionSurface = `${api}\n${hooks}\n${app}\n${panel}`;
    expect(app).toContain('currentView === "skills"');
    expect(app).toContain("skillsRef.current?.openImport()");
    for (const removed of [
      "getBackups",
      "deleteBackup",
      "installUnified",
      "restoreBackup",
      "discoverAvailable",
      "checkUpdates",
      "updateSkill",
      "migrateStorage",
      "searchSkillsSh",
      "getRepos",
      "addRepo",
      "removeRepo",
      "openZipFileDialog",
      "installFromZip",
      "SkillsPage",
      "skillsDiscovery",
      "openDiscovery",
      "openInstallFromZip",
      "openRestoreFromBackup",
      "SkillBackupEntry",
      "SkillUpdateInfo",
      "SkillConfirmation",
    ]) {
      expect(productionSurface, removed).not.toMatch(
        new RegExp(`\\b${removed}\\b`),
      );
    }
  });

  it("keeps one Chinese locale without online Skill and storage copy", () => {
    const localeNames = readdirSync(resolve(root, "src/i18n/locales"));
    expect(localeNames).toEqual(["zh.json"]);
    for (const localeName of ["zh"]) {
      const locale = JSON.parse(
        read(`src/i18n/locales/${localeName}.json`),
      ) as Record<string, any>;

      for (const key of [
        "install",
        "update",
        "repo",
        "skillssh",
        "restoreFromBackup",
        "installFromZip",
        "discover",
      ]) {
        expect(
          locale.skills[key],
          `${localeName}: skills.${key}`,
        ).toBeUndefined();
      }
      expect(locale.settings.skillStorage, localeName).toBeUndefined();
      expect(locale.settings.skillSync, localeName).toBeUndefined();
      expect(locale.deeplink?.skill, localeName).toBeUndefined();
      expect(locale.migration?.skillsSuccess, localeName).toBeUndefined();
    }
  });
});
