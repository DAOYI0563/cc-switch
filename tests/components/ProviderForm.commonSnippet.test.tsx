import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ProviderForm } from "@/components/providers/forms/ProviderForm";

const configMocks = vi.hoisted(() => ({
  getCommonConfigSnippet: vi.fn(),
  setCommonConfigSnippet: vi.fn(),
  extractCommonConfigSnippet: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  configApi: configMocks,
}));

vi.mock("@/components/providers/forms/BasicFormFields", () => ({
  BasicFormFields: () => null,
}));

vi.mock("@/components/JsonEditor", () => ({
  default: ({
    value,
    onChange,
  }: {
    value: string;
    onChange: (value: string) => void;
  }) => (
    <textarea
      aria-label="json-editor"
      value={value}
      onChange={(event) => onChange(event.target.value)}
    />
  ),
}));

describe("ProviderForm common snippets", () => {
  beforeEach(() => {
    configMocks.getCommonConfigSnippet.mockResolvedValue('{"theme":"dark"}');
    configMocks.setCommonConfigSnippet.mockResolvedValue(undefined);
    configMocks.extractCommonConfigSnippet.mockResolvedValue(
      '{"theme":"from-live"}',
    );
  });

  it("edits, extracts from live, and persists the per-provider Claude toggle", async () => {
    const onSubmit = vi.fn();
    render(
      <ProviderForm
        appId="claude"
        submitLabel="save-provider"
        onSubmit={onSubmit}
        onCancel={vi.fn()}
        initialData={{
          name: "Claude custom",
          category: "custom",
          settingsConfig: { env: {} },
          meta: { customUserAgent: "keep-me", commonConfigEnabled: false },
        }}
      />,
    );

    await waitFor(() =>
      expect(configMocks.getCommonConfigSnippet).toHaveBeenCalledWith("claude"),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.commonConfig.extract" }),
    );
    await waitFor(() =>
      expect(configMocks.extractCommonConfigSnippet).toHaveBeenCalledWith(
        "claude",
      ),
    );

    fireEvent.click(
      screen.getByRole("button", { name: "providerForm.commonConfig.save" }),
    );
    await waitFor(() =>
      expect(configMocks.setCommonConfigSnippet).toHaveBeenCalledWith(
        "claude",
        '{"theme":"from-live"}',
      ),
    );

    fireEvent.click(
      screen.getByRole("switch", { name: "providerForm.commonConfig.apply" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "save-provider" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0].meta).toMatchObject({
      customUserAgent: "keep-me",
      commonConfigEnabled: true,
    });
  });

  it("does not expose common snippets for OpenCode", () => {
    render(
      <ProviderForm
        appId="opencode"
        submitLabel="save-provider"
        onSubmit={vi.fn()}
        onCancel={vi.fn()}
      />,
    );

    expect(
      screen.queryByRole("switch", {
        name: "providerForm.commonConfig.apply",
      }),
    ).not.toBeInTheDocument();
  });
});
