import { invoke } from "@tauri-apps/api/core";

export const LOCAL_SCAN_DOMAINS = [
  "provider",
  "mcp",
  "prompt",
  "skill",
] as const;

export type LocalScanDomain = (typeof LOCAL_SCAN_DOMAINS)[number];

export const localScanApi = {
  enterPage: (domain: LocalScanDomain): Promise<void> =>
    invoke("local_scan_enter_page", { domain }),
};
