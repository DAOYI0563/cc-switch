import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "@testing-library/jest-dom";

import { WebdavSyncSection } from "@/components/settings/WebdavSyncSection";
import type { WebDavSyncSettings } from "@/types";

const toastSuccessMock = vi.fn();
const toastErrorMock = vi.fn();

vi.mock("sonner", () => ({
  toast: {
    success: (...args: unknown[]) => toastSuccessMock(...args),
    error: (...args: unknown[]) => toastErrorMock(...args),
  },
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

vi.mock("@/components/ui/button", () => ({
  Button: ({ children, ...props }: any) => (
    <button {...props}>{children}</button>
  ),
}));

vi.mock("@/components/ui/input", () => ({
  Input: (props: any) => <input {...props} />,
}));

vi.mock("@/components/ui/label", () => ({
  Label: ({ children, ...props }: any) => <label {...props}>{children}</label>,
}));

const { settingsApiMock } = vi.hoisted(() => ({
  settingsApiMock: {
    webdavTestConnection: vi.fn(),
    webdavSyncSaveSettings: vi.fn(),
  },
}));

vi.mock("@/lib/api", () => ({
  settingsApi: settingsApiMock,
}));

const baseConfig: WebDavSyncSettings = {
  baseUrl: "https://dav.example.com/dav/",
  username: "alice",
  password: "",
  remoteRoot: "cc-switch-sync",
  profile: "default",
};

function renderSection(config?: WebDavSyncSettings) {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });
  return render(
    <QueryClientProvider client={client}>
      <WebdavSyncSection config={config} />
    </QueryClientProvider>,
  );
}

describe("WebdavSyncSection", () => {
  beforeEach(() => {
    toastSuccessMock.mockReset();
    toastErrorMock.mockReset();
    settingsApiMock.webdavTestConnection.mockReset();
    settingsApiMock.webdavSyncSaveSettings.mockReset();
    settingsApiMock.webdavSyncSaveSettings.mockResolvedValue({ success: true });
    settingsApiMock.webdavTestConnection.mockResolvedValue({
      success: true,
      message: "ok",
    });
  });

  it("does not perform network or persistence work on render", () => {
    renderSection(baseConfig);

    expect(settingsApiMock.webdavTestConnection).not.toHaveBeenCalled();
    expect(settingsApiMock.webdavSyncSaveSettings).not.toHaveBeenCalled();
    expect(
      screen.queryByText("settings.webdavSync.upload"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("settings.webdavSync.download"),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByText("settings.webdavSync.autoSync"),
    ).not.toBeInTheDocument();
  });

  it("validates the required URL before saving", () => {
    renderSection({ ...baseConfig, baseUrl: "" });
    fireEvent.change(screen.getByLabelText("settings.webdavSync.username"), {
      target: { value: "bob" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    expect(toastErrorMock).toHaveBeenCalledWith(
      "settings.webdavSync.missingUrl",
    );
    expect(settingsApiMock.webdavSyncSaveSettings).not.toHaveBeenCalled();
  });

  it("saves credentials without implicitly testing the connection", async () => {
    renderSection(baseConfig);
    fireEvent.change(screen.getByLabelText("settings.webdavSync.remoteRoot"), {
      target: { value: "team-sync" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledWith(
        {
          baseUrl: "https://dav.example.com/dav/",
          username: "alice",
          password: "",
          remoteRoot: "team-sync",
          profile: "default",
        },
        false,
      );
    });
    expect(settingsApiMock.webdavTestConnection).not.toHaveBeenCalled();
    expect(toastSuccessMock).toHaveBeenCalledWith(
      "settings.webdavSync.saveSuccess",
    );
  });

  it("does not restore the saved password when the user explicitly clears it", async () => {
    renderSection({ ...baseConfig, password: "existing" });
    const password = screen.getByLabelText("settings.webdavSync.password");
    fireEvent.change(password, { target: { value: "" } });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.test" }),
    );

    await waitFor(() => {
      expect(settingsApiMock.webdavTestConnection).toHaveBeenCalledWith(
        expect.objectContaining({ password: "" }),
        false,
      );
    });
  });

  it("reports a settings persistence failure", async () => {
    settingsApiMock.webdavSyncSaveSettings.mockRejectedValueOnce(
      new Error("disk unavailable"),
    );
    renderSection(baseConfig);
    fireEvent.change(screen.getByLabelText("settings.webdavSync.profile"), {
      target: { value: "work" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "settings.webdavSync.save" }),
    );

    await waitFor(() => {
      expect(toastErrorMock).toHaveBeenCalledWith(
        "settings.webdavSync.saveFailed",
      );
    });
    expect(settingsApiMock.webdavSyncSaveSettings).toHaveBeenCalledTimes(1);
    expect(settingsApiMock.webdavTestConnection).not.toHaveBeenCalled();
  });
});
