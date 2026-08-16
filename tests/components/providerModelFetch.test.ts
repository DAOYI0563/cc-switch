import { describe, expect, it } from "vitest";
import { resolveProviderModelFetchTarget } from "@/components/providers/forms/providerModelFetch";

describe("resolveProviderModelFetchTarget", () => {
  it("extracts the current Claude target", () => {
    expect(
      resolveProviderModelFetchTarget(
        "claude",
        {
          env: {
            ANTHROPIC_BASE_URL: " https://claude.example/anthropic ",
            ANTHROPIC_AUTH_TOKEN: " token ",
          },
          modelsUrl: " https://claude.example/models ",
        },
        { isFullUrl: true, customUserAgent: " wsl-code-switch-test " },
      ),
    ).toEqual({
      baseUrl: "https://claude.example/anthropic",
      apiKey: "token",
      isFullUrl: true,
      modelsUrl: "https://claude.example/models",
      customUserAgent: "wsl-code-switch-test",
    });
  });

  it("extracts only the active Codex provider", () => {
    expect(
      resolveProviderModelFetchTarget("codex", {
        auth: { OPENAI_API_KEY: "sk-codex" },
        config: `model_provider = "active"
[model_providers.active]
base_url = "https://active.example/v1"
[model_providers.stale]
base_url = "https://stale.example/v1"
`,
      }),
    ).toMatchObject({
      baseUrl: "https://active.example/v1",
      apiKey: "sk-codex",
      isFullUrl: false,
      modelsUrl: undefined,
    });
  });

  it("extracts the OpenCode options target", () => {
    expect(
      resolveProviderModelFetchTarget("opencode", {
        options: {
          baseURL: "https://opencode.example/v1",
          apiKey: "sk-opencode",
          modelsURL: "https://opencode.example/models",
        },
      }),
    ).toEqual({
      baseUrl: "https://opencode.example/v1",
      apiKey: "sk-opencode",
      isFullUrl: false,
      modelsUrl: "https://opencode.example/models",
      customUserAgent: undefined,
    });
  });
});
