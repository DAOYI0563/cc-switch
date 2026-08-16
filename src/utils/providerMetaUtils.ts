import type { ProviderMeta } from "@/types";

export function normalizeProviderMeta(
  meta: ProviderMeta | Record<string, unknown> | undefined,
): ProviderMeta | undefined {
  if (!meta) return undefined;
  const retained: ProviderMeta = {};
  if (typeof meta.commonConfigEnabled === "boolean") {
    retained.commonConfigEnabled = meta.commonConfigEnabled;
  }
  if (typeof meta.isFullUrl === "boolean") {
    retained.isFullUrl = meta.isFullUrl;
  }
  if (typeof meta.customUserAgent === "string") {
    retained.customUserAgent = meta.customUserAgent;
  }
  return Object.keys(retained).length > 0 ? retained : undefined;
}
