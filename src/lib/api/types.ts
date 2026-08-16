/** The complete production client registry. Keep this aligned with ManagedClientId. */
export const MANAGED_APP_IDS = ["claude", "codex", "opencode"] as const;

export type ManagedAppId = (typeof MANAGED_APP_IDS)[number];

/** Enablement state shared by every resource that targets managed clients. */
export type ManagedClientApps = Record<ManagedAppId, boolean>;

export function isManagedAppId(value: unknown): value is ManagedAppId {
  return (
    typeof value === "string" && MANAGED_APP_IDS.includes(value as ManagedAppId)
  );
}

export function readStoredManagedAppId(
  storage: Pick<Storage, "getItem">,
  key: string,
): ManagedAppId {
  const stored = storage.getItem(key);
  return isManagedAppId(stored) ? stored : "claude";
}
