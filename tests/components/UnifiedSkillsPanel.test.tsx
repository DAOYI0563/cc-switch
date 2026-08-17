import { createRef } from "react";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import UnifiedSkillsPanel, {
  type UnifiedSkillsPanelHandle,
} from "@/components/skills/UnifiedSkillsPanel";
import type {
  InstalledSkill,
  LocalSkillScanResult,
  UnmanagedSkill,
} from "@/lib/api/skills";

const mocks = vi.hoisted(() => ({
  useScanUnmanagedSkills: vi.fn(),
  scanUnmanaged: vi.fn(),
  toggleApp: vi.fn(),
  bulkToggleApp: vi.fn(),
  syncFromLive: vi.fn(),
  uninstall: vi.fn(),
  importFromApps: vi.fn(),
  toastError: vi.fn(),
  toastSuccess: vi.fn(),
  toastWarning: vi.fn(),
}));

let installedSkills: InstalledSkill[] = [];
let unmanagedSkills: UnmanagedSkill[] = [];
let cachedScanResult: LocalSkillScanResult | undefined;

vi.mock("sonner", () => ({
  toast: {
    error: mocks.toastError,
    success: mocks.toastSuccess,
    warning: mocks.toastWarning,
  },
}));

vi.mock("@/hooks/useSkills", () => ({
  useInstalledSkills: () => ({ data: installedSkills, isLoading: false }),
  useToggleSkillApp: () => ({
    mutateAsync: mocks.toggleApp,
    isPending: false,
    variables: undefined,
  }),
  useBulkToggleSkillApp: () => ({
    mutateAsync: mocks.bulkToggleApp,
    isPending: false,
    variables: undefined,
  }),
  useSyncSkillFromLive: () => ({
    mutateAsync: mocks.syncFromLive,
    isPending: false,
    variables: undefined,
  }),
  useUninstallSkill: () => ({
    mutateAsync: mocks.uninstall,
    isPending: false,
  }),
  useImportSkillsFromApps: () => ({
    mutateAsync: mocks.importFromApps,
    isPending: false,
  }),
  useScanUnmanagedSkills: mocks.useScanUnmanagedSkills,
}));

type SkillOverrides = Omit<Partial<InstalledSkill>, "apps"> & {
  apps?: Partial<InstalledSkill["apps"]>;
};

function makeSkill(overrides: SkillOverrides = {}): InstalledSkill {
  const { apps, ...rest } = overrides;
  return {
    id: "alpha-id",
    name: "Alpha Skill",
    description: "Alpha description",
    directory: "alpha-directory",
    contentHash: "alpha-hash",
    totalSizeBytes: 1024,
    fileCount: 2,
    apps: {
      claude: false,
      codex: true,
      opencode: false,
      ...apps,
    },
    cloudEligible: true,
    createdAtMs: 1,
    updatedAtMs: 2,
    ...rest,
  };
}

function makeScanResult(
  overrides: Partial<LocalSkillScanResult> = {},
): LocalSkillScanResult {
  return {
    installed: installedSkills,
    unmanaged: unmanagedSkills,
    issues: [],
    updatedCount: 0,
    removedCount: 0,
    ...overrides,
  };
}

function resolveScan(result: LocalSkillScanResult) {
  installedSkills = result.installed;
  cachedScanResult = result;
  return Promise.resolve({ data: result });
}

function renderPanel(
  props: Partial<React.ComponentProps<typeof UnifiedSkillsPanel>> = {},
) {
  return render(<UnifiedSkillsPanel currentApp="claude" {...props} />);
}

describe("UnifiedSkillsPanel local-only management", () => {
  beforeEach(() => {
    installedSkills = [];
    cachedScanResult = undefined;
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        description: "Found in OpenCode",
        foundIn: ["opencode"],
      },
    ];
    for (const mock of Object.values(mocks)) mock.mockReset();
    mocks.useScanUnmanagedSkills.mockImplementation(() => ({
      data: cachedScanResult,
      refetch: mocks.scanUnmanaged,
    }));
    mocks.scanUnmanaged.mockImplementation(() => resolveScan(makeScanResult()));
    mocks.toggleApp.mockResolvedValue(makeSkill());
    mocks.bulkToggleApp.mockResolvedValue({ succeeded: [], failed: [] });
    mocks.syncFromLive.mockResolvedValue(makeSkill());
    mocks.uninstall.mockResolvedValue(true);
    mocks.importFromApps.mockResolvedValue([makeSkill()]);
  });

  it("shows a local import empty state instead of repository discovery", () => {
    renderPanel();

    expect(screen.getByText("skills.noInstalled")).toBeInTheDocument();
    expect(
      screen.getByText("skills.noInstalledDescription"),
    ).toBeInTheDocument();
    expect(screen.queryByText("skills.discover")).not.toBeInTheDocument();
    expect(screen.queryByText("skills.checkUpdates")).not.toBeInTheDocument();
  });

  it("offers local import without scanning automatically on page mount", async () => {
    renderPanel();

    expect(mocks.useScanUnmanagedSkills).toHaveBeenCalledWith({
      enabled: false,
    });
    expect(mocks.scanUnmanaged).not.toHaveBeenCalled();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "skills.import" }));

    expect(mocks.scanUnmanaged).toHaveBeenCalledTimes(1);
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("opens the import dialog with progress while the WSL scan is running", async () => {
    let finishScan:
      | ((value: { data: LocalSkillScanResult }) => void)
      | undefined;
    mocks.scanUnmanaged.mockImplementationOnce(
      () =>
        new Promise<{ data: LocalSkillScanResult }>((resolve) => {
          finishScan = resolve;
        }),
    );
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "skills.import" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("skills.scanLoading")).toBeInTheDocument();
    expect(dialog.querySelector(".animate-spin")).toBeInTheDocument();

    await act(async () => finishScan?.({ data: makeScanResult() }));
    expect(within(dialog).getByText("Shared Skill")).toBeInTheDocument();
  });

  it("removes managed rows immediately when a scan returns no installed Skills", async () => {
    installedSkills = [makeSkill()];
    unmanagedSkills = [];
    mocks.scanUnmanaged.mockImplementationOnce(() =>
      resolveScan(makeScanResult({ installed: [], removedCount: 1 })),
    );
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    expect(screen.getByText("Alpha Skill")).toBeInTheDocument();
    await act(async () => {
      ref.current?.openImport();
    });

    await waitFor(() => {
      expect(screen.queryByText("Alpha Skill")).not.toBeInTheDocument();
      expect(screen.getByText("skills.noInstalled")).toBeInTheDocument();
    });
    expect(mocks.toastSuccess).toHaveBeenCalledWith("skills.scanResult", {
      closeButton: true,
    });
  });

  it("uses scanned client markers and metadata even when nothing is importable", async () => {
    installedSkills = [makeSkill()];
    unmanagedSkills = [];
    const refreshed = makeSkill({
      description: "Refreshed description",
      totalSizeBytes: 2048,
      fileCount: 7,
      updatedAtMs: 86_400_000,
      apps: { claude: true, codex: false, opencode: false },
    });
    mocks.scanUnmanaged.mockImplementationOnce(() =>
      resolveScan(makeScanResult({ installed: [refreshed], updatedCount: 1 })),
    );
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });

    const row = await screen.findByText("Refreshed description");
    const listItem = row.closest<HTMLElement>(".group")!;
    expect(within(listItem).getByText(/2\.0 KB/)).toBeInTheDocument();
    expect(
      within(listItem).getByRole("button", { name: "Claude" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      within(listItem).getByRole("button", { name: "Codex" }),
    ).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it.each([
    ["divergent_copies", "divergentCopies"],
    ["invalid_copy", "invalidCopy"],
    ["case_collision", "caseCollision"],
  ] as const)(
    "keeps a %s issue row visible, marks it, and blocks removal",
    async (kind, translationKey) => {
      installedSkills = [makeSkill()];
      unmanagedSkills = [];
      mocks.scanUnmanaged.mockImplementationOnce(() =>
        resolveScan(
          makeScanResult({
            installed: installedSkills,
            issues: [
              {
                directory: "alpha-directory",
                clients: ["claude", "codex"],
                kind,
              },
            ],
          }),
        ),
      );
      const ref = createRef<UnifiedSkillsPanelHandle>();
      render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

      await act(async () => {
        ref.current?.openImport();
      });

      expect(await screen.findByText("Alpha Skill")).toBeInTheDocument();
      expect(
        screen.getByText(`skills.issues.${translationKey}.badge`),
      ).toBeInTheDocument();
      expect(
        screen.getByRole("button", { name: "skills.uninstall" }),
      ).toBeDisabled();
      expect(mocks.toastWarning).toHaveBeenCalledWith("skills.scanResult", {
        closeButton: true,
      });
    },
  );

  it("keeps a standalone case collision visible when no Skill row is importable", async () => {
    installedSkills = [];
    unmanagedSkills = [];
    mocks.scanUnmanaged.mockImplementationOnce(() =>
      resolveScan(
        makeScanResult({
          issues: [
            {
              directory: "Foo",
              clients: ["claude", "codex"],
              kind: "case_collision",
            },
          ],
        }),
      ),
    );
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });

    const issues = await screen.findByRole("region", {
      name: "skills.scanIssuesTitle",
    });
    expect(within(issues).getByText("Foo")).toBeInTheDocument();
    expect(
      within(issues).getByText("skills.issues.caseCollision.badge"),
    ).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("restores scan issues from the retained query result after remount", () => {
    cachedScanResult = makeScanResult({
      issues: [
        {
          directory: "cached-collision",
          clients: ["opencode"],
          kind: "case_collision",
        },
      ],
    });

    const first = renderPanel();
    first.unmount();
    renderPanel();

    const issues = screen.getByRole("region", {
      name: "skills.scanIssuesTitle",
    });
    expect(within(issues).getByText("cached-collision")).toBeInTheDocument();
  });

  it("keeps the previous managed list when scanning fails", async () => {
    installedSkills = [makeSkill()];
    mocks.scanUnmanaged.mockRejectedValueOnce(new Error("scan failed"));
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });

    expect(await screen.findByText("Alpha Skill")).toBeInTheDocument();
    expect(mocks.toastError).toHaveBeenCalledWith("skills.scanFailed", {
      description: "skills.scanFailedDescription",
    });
  });

  it.each([
    ["name", "Alpha Skill"],
    ["id", "alpha-id"],
    ["description", "Alpha description"],
    ["directory", "alpha-directory"],
  ])("filters local Skills by %s", async (_field, query) => {
    installedSkills = [
      makeSkill(),
      makeSkill({
        id: "beta",
        name: "Beta",
        description: "Beta description",
        directory: "beta-directory",
      }),
    ];
    renderPanel();

    await userEvent.setup().type(
      screen.getByRole("textbox", {
        name: "skills.installedSearchAriaLabel",
      }),
      query,
    );

    expect(screen.getByText("Alpha Skill")).toBeInTheDocument();
    expect(screen.queryByText("Beta")).not.toBeInTheDocument();
  });

  it("bulk toggles the full list using an existing live source", async () => {
    installedSkills = [
      makeSkill({ id: "alpha", apps: { codex: true } }),
      makeSkill({
        id: "beta",
        name: "Beta",
        apps: { codex: false, opencode: true },
      }),
    ];
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByText("Claude:").closest("button")!);

    await waitFor(() => {
      expect(mocks.bulkToggleApp).toHaveBeenCalledWith({
        ids: ["alpha", "beta"],
        app: "claude",
        sourceApps: { alpha: "codex", beta: "opencode" },
        enabled: true,
      });
    });
  });

  it("toggles one client from the first enabled local source", async () => {
    installedSkills = [makeSkill({ apps: { codex: true } })];
    renderPanel();

    const row = screen.getByText("Alpha Skill").closest<HTMLElement>(".group")!;
    await userEvent
      .setup()
      .click(within(row).getByRole("button", { name: "OpenCode" }));

    await waitFor(() => {
      expect(mocks.toggleApp).toHaveBeenCalledWith({
        id: "alpha-id",
        app: "opencode",
        sourceApp: "codex",
        enabled: true,
      });
    });
  });

  it("offers explicit live sync only for the current enabled client", async () => {
    installedSkills = [makeSkill({ apps: { claude: true, codex: true } })];
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "skills.syncFromLive" }));

    await waitFor(() => {
      expect(mocks.syncFromLive).toHaveBeenCalledWith({
        id: "alpha-id",
        sourceApp: "claude",
      });
    });
  });

  it("imports scanned local content with an explicit source and target apps", async () => {
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Shared Skill")).toBeInTheDocument();
    await userEvent
      .setup()
      .click(
        within(dialog).getByRole("button", { name: "skills.importSelected" }),
      );

    await waitFor(() => {
      expect(mocks.importFromApps).toHaveBeenCalledWith([
        {
          directory: "shared-skill",
          sourceClient: "opencode",
          apps: { claude: false, codex: false, opencode: true },
        },
      ]);
    });

    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    await act(async () => {
      ref.current?.openImport();
    });
    expect(mocks.scanUnmanaged).toHaveBeenCalledTimes(2);
  });

  it("defaults a multi-client Skill to every client where it was found", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        description: "Different copies may exist",
        foundIn: ["claude", "codex", "opencode"],
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    await userEvent
      .setup()
      .click(
        within(dialog).getByRole("button", { name: "skills.importSelected" }),
      );

    await waitFor(() => {
      expect(mocks.importFromApps).toHaveBeenCalledWith([
        {
          directory: "shared-skill",
          sourceClient: "claude",
          apps: { claude: true, codex: true, opencode: true },
        },
      ]);
    });
  });

  it("keeps the source enabled while allowing explicit target selection", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        foundIn: ["claude", "codex", "opencode"],
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    const claudeToggle = within(dialog).getByRole("button", { name: "Claude" });
    const codexToggle = within(dialog).getByRole("button", { name: "Codex" });
    const opencodeToggle = within(dialog).getByRole("button", {
      name: "OpenCode",
    });
    expect(claudeToggle).toHaveAttribute("aria-pressed", "true");
    expect(codexToggle).toHaveAttribute("aria-pressed", "true");
    expect(opencodeToggle).toHaveAttribute("aria-pressed", "true");

    const user = userEvent.setup();
    await user.click(claudeToggle);
    expect(claudeToggle).toHaveAttribute("aria-pressed", "true");
    await user.click(codexToggle);
    await user.click(opencodeToggle);
    await user.click(
      within(dialog).getByRole("button", { name: "skills.importSelected" }),
    );

    await waitFor(() => {
      expect(mocks.importFromApps).toHaveBeenCalledWith([
        {
          directory: "shared-skill",
          sourceClient: "claude",
          apps: { claude: true, codex: false, opencode: false },
        },
      ]);
    });
  });

  it("keeps explicit target selection when the content source changes", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        foundIn: ["claude", "codex"],
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("button", { name: "OpenCode" }));
    await user.selectOptions(
      within(dialog).getByRole("combobox", {
        name: "skills.sourceClient: Shared Skill",
      }),
      "codex",
    );
    expect(
      within(dialog).getByRole("button", { name: "Claude" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      within(dialog).getByRole("button", { name: "Codex" }),
    ).toHaveAttribute("aria-pressed", "true");
    expect(
      within(dialog).getByRole("button", { name: "OpenCode" }),
    ).toHaveAttribute("aria-pressed", "true");
    await user.click(
      within(dialog).getByRole("button", { name: "skills.importSelected" }),
    );

    await waitFor(() => {
      expect(mocks.importFromApps).toHaveBeenCalledWith([
        {
          directory: "shared-skill",
          sourceClient: "codex",
          apps: { claude: true, codex: true, opencode: true },
        },
      ]);
    });
  });

  it("deletes from all enabled clients without exposing a backup result", async () => {
    installedSkills = [makeSkill()];
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "skills.uninstall" }));
    const dialog = screen.getByRole("dialog");
    await userEvent
      .setup()
      .click(within(dialog).getByRole("button", { name: "skills.uninstall" }));

    await waitFor(() =>
      expect(mocks.uninstall).toHaveBeenCalledWith("alpha-id"),
    );
    expect(mocks.toastSuccess).toHaveBeenCalledWith("skills.uninstallSuccess", {
      closeButton: true,
    });
  });

  it("marks oversized local Skills as unavailable for cloud sync", () => {
    installedSkills = [makeSkill({ cloudEligible: false })];
    renderPanel();

    expect(screen.getByText("skills.localOnly")).toBeInTheDocument();
    expect(
      screen.getByTitle("skills.localOnlyDescription"),
    ).toBeInTheDocument();
  });

  it("opens a read-only detail dialog with per-client status and paths", async () => {
    installedSkills = [
      makeSkill({
        apps: { claude: false, codex: true, opencode: false },
      }),
    ];
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Alpha Skill" }));

    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Alpha Skill")).toBeInTheDocument();
    expect(
      within(dialog).getByText("skills.detailPreviewTitle"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("skills.detailAgentEnabled"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("~/.codex/skills/alpha-directory"),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("alpha-hash")).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", {
        name: "skills.detailOpenDirectory",
      }),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("skills.detailPreviewEmpty"),
    ).toBeInTheDocument();
  });
});
