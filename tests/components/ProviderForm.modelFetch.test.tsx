import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";

const modelFetchMocks = vi.hoisted(() => ({
  fetchModelsForConfig: vi.fn(),
  showFetchModelsError: vi.fn(),
}));

vi.mock("@/lib/api/model-fetch", () => ({
  fetchModelsForConfig: modelFetchMocks.fetchModelsForConfig,
  showFetchModelsError: modelFetchMocks.showFetchModelsError,
}));

vi.mock("@/components/providers/forms/BasicFormFields", () => ({
  BasicFormFields: () => null,
}));

vi.mock("@/components/JsonEditor", () => ({
  default: ({ value }: { value: string }) => <pre>{value}</pre>,
}));

describe("ProviderForm manual model fetch", () => {
  beforeEach(() => {
    modelFetchMocks.fetchModelsForConfig.mockResolvedValue([
      { id: "model-b", ownedBy: null },
      { id: "model-a", ownedBy: "vendor" },
    ]);
  });

  it("only fetches the current Claude target after an explicit click", async () => {
    render(
      <ProviderForm
        appId="claude"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        initialData={{
          name: "Custom Claude",
          category: "custom",
          settingsConfig: {
            env: {
              ANTHROPIC_BASE_URL: "https://claude.example/anthropic",
              ANTHROPIC_AUTH_TOKEN: "sk-claude",
            },
          },
          meta: {
            isFullUrl: false,
            customUserAgent: "provider-test",
          },
        }}
      />,
    );

    expect(modelFetchMocks.fetchModelsForConfig).not.toHaveBeenCalled();

    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    await waitFor(() => {
      expect(modelFetchMocks.fetchModelsForConfig).toHaveBeenCalledTimes(1);
    });
    expect(modelFetchMocks.fetchModelsForConfig).toHaveBeenCalledWith(
      "https://claude.example/anthropic",
      "sk-claude",
      false,
      undefined,
      "provider-test",
    );
    expect(screen.getByText("model-a")).toBeInTheDocument();
    expect(screen.getByText("model-b")).toBeInTheDocument();
  });

  it("rejects missing config locally without invoking the backend", () => {
    render(
      <ProviderForm
        appId="opencode"
        submitLabel="save"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
        initialData={{
          name: "Incomplete OpenCode",
          category: "custom",
          settingsConfig: { options: { baseURL: "", apiKey: "" } },
        }}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.fetchModels" }),
    );

    expect(modelFetchMocks.fetchModelsForConfig).not.toHaveBeenCalled();
    expect(modelFetchMocks.showFetchModelsError).toHaveBeenCalledWith(
      null,
      expect.anything(),
      { hasApiKey: false, hasBaseUrl: false },
    );
  });
});
