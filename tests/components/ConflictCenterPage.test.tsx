import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ConflictCenterPage } from "@/components/conflicts/ConflictCenterPage";
import {
  LOCAL_SCAN_DOMAINS,
  localScanApi,
  type ConflictCenterItem,
} from "@/lib/api";

const mocks = vi.hoisted(() => ({
  refetch: vi.fn(),
  resolve: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  items: [] as ConflictCenterItem[],
}));

vi.mock("@/lib/query", () => ({
  useConflictCenterItemsQuery: () => ({
    data: mocks.items,
    isLoading: false,
    isError: false,
    error: null,
    isRefetching: false,
    refetch: mocks.refetch,
  }),
  useResolveConflictCenterItemMutation: () => ({
    mutate: mocks.resolve,
    isPending: false,
    variables: undefined,
  }),
}));

vi.mock("@/components/ConfirmDialog", () => ({
  ConfirmDialog: ({
    isOpen,
    title,
    message,
    confirmText,
    onConfirm,
    onCancel,
  }: {
    isOpen: boolean;
    title: string;
    message: string;
    confirmText?: string;
    onConfirm: (checked: boolean) => void;
    onCancel: () => void;
  }) =>
    isOpen ? (
      <div role="dialog">
        <span>{title}</span>
        <span>{message}</span>
        <button type="button" onClick={() => onConfirm(false)}>
          confirm-resolution:{confirmText}
        </button>
        <button type="button" onClick={onCancel}>
          cancel-resolution
        </button>
      </div>
    ) : null,
}));

vi.mock("sonner", () => ({
  toast: {
    success: mocks.toastSuccess,
    error: mocks.toastError,
  },
}));

const item: ConflictCenterItem = {
  schemaVersion: 1,
  itemId: "local_provider_claude_alpha",
  source: "local_scan",
  domain: "provider",
  clientId: "claude",
  recordId: "alpha",
  displayName: "Alpha Provider",
  modifiedAtMs: Date.UTC(2026, 7, 14, 0, 0, 0),
  disposition: { type: "conflict", kind: "concurrent_update" },
  baselineDigest: "a".repeat(64),
  localDigest: "b".repeat(64),
  externalDigest: "c".repeat(64),
  actions: ["accept_external", "keep_local", "retry"],
};

const enterPageSpy = vi.spyOn(localScanApi, "enterPage");

describe("ConflictCenterPage", () => {
  beforeEach(() => {
    mocks.items = [item];
    mocks.refetch.mockReset();
    mocks.refetch.mockResolvedValue(undefined);
    mocks.resolve.mockReset();
    mocks.toastSuccess.mockReset();
    mocks.toastError.mockReset();
    enterPageSpy.mockReset();
    enterPageSpy.mockResolvedValue(undefined);
  });

  it("renders redacted summaries and requests all local domain scans", async () => {
    render(<ConflictCenterPage />);

    expect(screen.getByText("Alpha Provider")).toBeInTheDocument();
    expect(
      screen.getByText("conflictCenter.sources.local_scan"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("conflictCenter.domains.provider"),
    ).toBeInTheDocument();
    expect(
      screen.getByText(
        "conflictCenter.dispositions.conflict.concurrent_update",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText(/alpha/)).toBeInTheDocument();
    expect(screen.getByText("aaaaaaaaaaaa...aaaaaaaa")).toBeInTheDocument();
    expect(screen.getByText("bbbbbbbbbbbb...bbbbbbbb")).toBeInTheDocument();
    expect(screen.getByText("cccccccccccc...cccccccc")).toBeInTheDocument();

    await waitFor(() => expect(enterPageSpy).toHaveBeenCalledTimes(4));
    expect(enterPageSpy.mock.calls.map(([domain]) => domain)).toEqual(
      LOCAL_SCAN_DOMAINS,
    );
    await waitFor(() => expect(mocks.refetch).toHaveBeenCalledTimes(1));
  });

  it("requires confirmation for writes but retries immediately", () => {
    render(<ConflictCenterPage />);

    fireEvent.click(
      screen.getByRole("button", {
        name: "conflictCenter.actions.acceptWsl",
      }),
    );
    expect(mocks.resolve).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /confirm-resolution/ }));
    expect(mocks.resolve).toHaveBeenCalledWith(
      { item, action: "accept_external" },
      expect.objectContaining({
        onSuccess: expect.any(Function),
        onError: expect.any(Function),
      }),
    );

    mocks.resolve.mockClear();
    fireEvent.click(
      screen.getByRole("button", { name: "conflictCenter.actions.retry" }),
    );
    expect(mocks.resolve).toHaveBeenCalledWith(
      { item, action: "retry" },
      expect.any(Object),
    );
  });
});
