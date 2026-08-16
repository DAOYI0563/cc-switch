import { parse as parseToml } from "smol-toml";

type TomlRecord = Record<string, unknown>;

const asRecord = (value: unknown): TomlRecord | undefined =>
  value != null && typeof value === "object" && !Array.isArray(value)
    ? (value as TomlRecord)
    : undefined;

const asText = (value: unknown): string | undefined =>
  typeof value === "string" && value.trim() ? value.trim() : undefined;

function selectedProvider(configText: string): {
  root: TomlRecord;
  provider?: TomlRecord;
} {
  try {
    const root = parseToml(configText) as TomlRecord;
    const providerName = asText(root.model_provider);
    const providers = asRecord(root.model_providers);
    return {
      root,
      provider: providerName ? asRecord(providers?.[providerName]) : undefined,
    };
  } catch {
    return { root: {} };
  }
}

export const extractCodexBaseUrl = (
  configText: string | undefined | null,
): string | undefined => {
  const { root, provider } = selectedProvider(configText ?? "");
  return asText(provider?.base_url) ?? asText(root.base_url);
};

export const extractCodexExperimentalBearerToken = (
  configText: string | undefined | null,
): string | undefined => {
  const { root, provider } = selectedProvider(configText ?? "");
  return (
    asText(provider?.experimental_bearer_token) ??
    asText(root.experimental_bearer_token)
  );
};
