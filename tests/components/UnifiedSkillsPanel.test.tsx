import { createRef } from "react";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import UnifiedSkillsPanel, {
  type UnifiedSkillsPanelHandle,
} from "@/components/skills/UnifiedSkillsPanel";
import type { InstalledSkill, UnmanagedSkill } from "@/lib/api/skills";

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
}));

let installedSkills: InstalledSkill[] = [];
let unmanagedSkills: UnmanagedSkill[] = [];

vi.mock("sonner", () => ({
  toast: {
    error: mocks.toastError,
    success: mocks.toastSuccess,
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

function renderPanel(
  props: Partial<React.ComponentProps<typeof UnifiedSkillsPanel>> = {},
) {
  return render(<UnifiedSkillsPanel currentApp="claude" {...props} />);
}

describe("UnifiedSkillsPanel local-only management", () => {
  beforeEach(() => {
    installedSkills = [];
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        description: "Found in OpenCode",
        foundIn: ["opencode"],
        path: "/home/zhldm/.config/opencode/skills/shared-skill",
      },
    ];
    for (const mock of Object.values(mocks)) mock.mockReset();
    mocks.useScanUnmanagedSkills.mockImplementation(() => ({
      data: unmanagedSkills,
      refetch: mocks.scanUnmanaged,
    }));
    mocks.scanUnmanaged.mockResolvedValue({ data: unmanagedSkills });
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
    let finishScan: ((value: { data: UnmanagedSkill[] }) => void) | undefined;
    mocks.scanUnmanaged.mockImplementationOnce(
      () =>
        new Promise<{ data: UnmanagedSkill[] }>((resolve) => {
          finishScan = resolve;
        }),
    );
    renderPanel();

    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "skills.import" }));

    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("skills.scanLoading")).toBeInTheDocument();

    await act(async () => finishScan?.({ data: unmanagedSkills }));
    expect(within(dialog).getByText("Shared Skill")).toBeInTheDocument();
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
  });

  it("defaults a multi-client Skill to only its explicit content source", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        description: "Different copies may exist",
        foundIn: ["claude", "codex"],
        path: "/home/zhldm/.claude/skills/shared-skill",
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
          apps: { claude: true, codex: false, opencode: false },
        },
      ]);
    });
  });

  it("preselects hash-identical copies and leaves divergent ones off", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        foundIn: ["claude", "codex", "opencode"],
        copies: [
          { client: "claude", contentHash: "hash-a" },
          { client: "codex", contentHash: "hash-a" },
          { client: "opencode", contentHash: "hash-b" },
        ],
        path: "/home/zhldm/.claude/skills/shared-skill",
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("skills.importCopiesConflict"),
    ).toBeInTheDocument();

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
          apps: { claude: true, codex: true, opencode: false },
        },
      ]);
    });
  });

  it("marks fully identical copies and enables every found client", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        foundIn: ["claude", "codex"],
        copies: [
          { client: "claude", contentHash: "hash-a" },
          { client: "codex", contentHash: "hash-a" },
        ],
        path: "/home/zhldm/.claude/skills/shared-skill",
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    expect(
      within(dialog).getByText("skills.importCopiesConsistent"),
    ).toBeInTheDocument();

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
          apps: { claude: true, codex: true, opencode: false },
        },
      ]);
    });
  });

  it("resets import targets when the explicit content source changes", async () => {
    unmanagedSkills = [
      {
        directory: "shared-skill",
        name: "Shared Skill",
        foundIn: ["claude", "codex"],
        path: "/home/zhldm/.claude/skills/shared-skill",
      },
    ];
    const ref = createRef<UnifiedSkillsPanelHandle>();
    render(<UnifiedSkillsPanel ref={ref} currentApp="claude" />);

    await act(async () => {
      ref.current?.openImport();
    });
    const dialog = await screen.findByRole("dialog");
    await userEvent.setup().selectOptions(
      within(dialog).getByRole("combobox", {
        name: "skills.sourceClient: Shared Skill",
      }),
      "codex",
    );
    await userEvent
      .setup()
      .click(
        within(dialog).getByRole("button", { name: "skills.importSelected" }),
      );

    await waitFor(() => {
      expect(mocks.importFromApps).toHaveBeenCalledWith([
        {
          directory: "shared-skill",
          sourceClient: "codex",
          apps: { claude: false, codex: true, opencode: false },
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
    expect(within(dialog).getByText("skills.detailPreviewTitle")).toBeInTheDocument();
    expect(
      within(dialog).getByText("skills.detailAgentEnabled"),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByText("~/.codex/skills/alpha-directory"),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("alpha-hash")).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", { name: "skills.detailOpenDirectory" }),
    ).toBeInTheDocument();
    expect(within(dialog).getByText("skills.detailPreviewEmpty")).toBeInTheDocument();
  });
});
