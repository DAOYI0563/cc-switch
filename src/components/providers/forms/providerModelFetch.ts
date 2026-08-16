import type { ManagedAppId } from "@/lib/api";
import type { ProviderMeta } from "@/types";
import {
  extractCodexBaseUrl,
  extractCodexExperimentalBearerToken,
} from "@/utils/providerConfigUtils";

export interface ProviderModelFetchTarget {
  baseUrl: string;
  apiKey: string;
  isFullUrl: boolean;
  modelsUrl?: string;
  customUserAgent?: string;
}

const asRecord = (value: unknown): Record<string, unknown> | undefined =>
  value != null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;

const text = (value: unknown): string =>
  typeof value === "string" ? value.trim() : "";

const firstText = (...values: unknown[]): string => {
  for (const value of values) {
    const candidate = text(value);
    if (candidate) return candidate;
  }
  return "";
};

export function resolveProviderModelFetchTarget(
  appId: ManagedAppId,
  settingsConfig: Record<string, unknown>,
  meta?: ProviderMeta,
): ProviderModelFetchTarget {
  if (appId === "claude") {
    const env = asRecord(settingsConfig.env);
    return {
      baseUrl: text(env?.ANTHROPIC_BASE_URL),
      apiKey: firstText(
        env?.ANTHROPIC_AUTH_TOKEN,
        env?.ANTHROPIC_API_KEY,
        env?.OPENROUTER_API_KEY,
        env?.GOOGLE_API_KEY,
      ),
      isFullUrl: meta?.isFullUrl === true,
      modelsUrl: text(settingsConfig.modelsUrl) || undefined,
      customUserAgent: text(meta?.customUserAgent) || undefined,
    };
  }

  if (appId === "codex") {
    const auth = asRecord(settingsConfig.auth);
    const config = text(settingsConfig.config);
    return {
      baseUrl: extractCodexBaseUrl(config)?.trim() ?? "",
      apiKey: firstText(
        auth?.OPENAI_API_KEY,
        extractCodexExperimentalBearerToken(config),
      ),
      isFullUrl: meta?.isFullUrl === true,
      modelsUrl: text(settingsConfig.modelsUrl) || undefined,
      customUserAgent: text(meta?.customUserAgent) || undefined,
    };
  }

  const options = asRecord(settingsConfig.options);
  return {
    baseUrl: text(options?.baseURL),
    apiKey: text(options?.apiKey),
    isFullUrl: false,
    modelsUrl:
      firstText(options?.modelsURL, settingsConfig.modelsUrl) || undefined,
    customUserAgent: undefined,
  };
}
