import { useEffect } from "react";

import { localScanApi, type LocalScanDomain } from "@/lib/api";

export function useLocalScanPage(domain: LocalScanDomain | null): void {
  useEffect(() => {
    if (!domain) return;
    void localScanApi.enterPage(domain).catch((error) => {
      console.error("[local-scan] Failed to request page scan", error);
    });
  }, [domain]);
}
