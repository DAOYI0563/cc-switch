import { Database, ExternalLink, FolderOpen } from "lucide-react";

import { Button } from "@/components/ui/button";
import { settingsApi } from "@/lib/api";

const RELEASES_URL = "https://github.com/farion1231/cc-switch/releases";

export function DatabaseUpgrade({
  payload,
}: {
  payload: {
    path?: string;
    error?: string;
    db_version?: number;
    supported_version?: number;
  };
}) {
  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-6 text-foreground">
      <section className="w-full max-w-lg space-y-5 border border-border-default bg-card p-6">
        <div className="flex items-start gap-3">
          <Database className="mt-0.5 h-6 w-6 text-amber-600" />
          <div>
            <h1 className="text-base font-semibold">数据库版本过新</h1>
            <p className="mt-1 text-sm text-muted-foreground">
              当前便携版无法安全读取该数据库。请下载更新版本后重新打开，应用不会修改现有数据。
            </p>
          </div>
        </div>
        <div className="space-y-1 border-y border-border-default py-3 text-xs text-muted-foreground">
          {payload.error ? <p>{payload.error}</p> : null}
          {payload.path ? (
            <code className="block break-all">{payload.path}</code>
          ) : null}
          {payload.db_version != null && payload.supported_version != null ? (
            <p>
              数据库 v{payload.db_version}，当前支持 v
              {payload.supported_version}
            </p>
          ) : null}
        </div>
        <div className="flex justify-end gap-2">
          <Button
            variant="outline"
            onClick={() => void settingsApi.openAppConfigFolder()}
          >
            <FolderOpen className="mr-2 h-4 w-4" />
            打开数据目录
          </Button>
          <Button onClick={() => void settingsApi.openExternal(RELEASES_URL)}>
            <ExternalLink className="mr-2 h-4 w-4" />
            打开下载页
          </Button>
        </div>
      </section>
    </div>
  );
}
