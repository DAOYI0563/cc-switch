import { FolderLock } from "lucide-react";

import type { ResolvedDirectories } from "@/hooks/useSettings";

export function DirectorySettings({
  resolvedDirs,
}: {
  resolvedDirs: ResolvedDirectories;
}) {
  const rows = [
    ["Claude Code", resolvedDirs.claude],
    ["Codex", resolvedDirs.codex],
    ["OpenCode", resolvedDirs.opencode],
  ] as const;

  return (
    <section className="space-y-4">
      <header className="space-y-1">
        <div className="flex items-center gap-2">
          <FolderLock className="h-4 w-4 text-primary" />
          <h3 className="text-sm font-medium">固定 WSL 配置路径</h3>
        </div>
        <p className="text-xs text-muted-foreground">
          当前版本固定管理 Ubuntu 中 zhldm 用户的三个客户端配置。
        </p>
      </header>
      <div className="divide-y divide-border-default border-y border-border-default">
        {rows.map(([name, path]) => (
          <div key={name} className="grid gap-1 py-3 md:grid-cols-[120px_1fr]">
            <span className="text-xs font-medium">{name}</span>
            <code className="break-all text-xs text-muted-foreground">
              {path}
            </code>
          </div>
        ))}
      </div>
    </section>
  );
}
