import { invoke } from "@tauri-apps/api/core";

import type { ManagedAppId } from "./types";

export type ConflictCenterSource = "local_scan" | "webdav";

export type PortableDomain =
  | "provider"
  | "mcp"
  | "prompt"
  | "skill"
  | "common_snippet"
  | "daily_brief"
  | "portable_setting";

export type LocalDifferenceKind = "added" | "modified" | "deleted";

export type LocalConflictKind =
  | "ambiguous_local_match"
  | "concurrent_update"
  | "update_delete"
  | "delete_without_baseline"
  | "parse_failed"
  | "integrity_mismatch";

export type LocalScanFailureKind =
  | "not_found"
  | "permission_denied"
  | "invalid_path"
  | "path_outside_root"
  | "link_or_reparse_point"
  | "path_cycle"
  | "path_resolution_failed"
  | "read_failed"
  | "digest_failed"
  | "parse_failed";

export type ConflictCenterDisposition =
  | { type: "difference"; kind: LocalDifferenceKind }
  | { type: "conflict"; kind: LocalConflictKind };

export type ConflictResolutionAction =
  | "accept_external"
  | "keep_local"
  | "keep_both"
  | "retry";

export interface ConflictCenterItem {
  schemaVersion: number;
  itemId: string;
  source: ConflictCenterSource;
  domain: PortableDomain;
  clientId?: ManagedAppId;
  recordId?: string;
  displayName: string;
  modifiedAtMs?: number;
  disposition: ConflictCenterDisposition;
  baselineDigest?: string;
  localDigest?: string;
  externalDigest?: string;
  failureKind?: LocalScanFailureKind;
  actions: ConflictResolutionAction[];
}

export interface ConflictResolutionRequest {
  itemId: string;
  action: ConflictResolutionAction;
}

export const conflictCenterApi = {
  list: (): Promise<ConflictCenterItem[]> =>
    invoke("list_conflict_center_items_command"),

  resolve: async (request: ConflictResolutionRequest): Promise<void> => {
    await invoke("resolve_conflict_center_item_command", { request });
  },
};
