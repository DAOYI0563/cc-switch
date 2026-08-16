import type { ManagedAppId } from "@/lib/api/types";

/** Normalize an optional provider icon selected for a managed client. */
export function resolveProviderIcon(
  _appId: ManagedAppId,
  icon?: string,
  _iconColor?: string,
): string | undefined {
  const normalizedIcon = icon?.trim();
  return normalizedIcon || undefined;
}
