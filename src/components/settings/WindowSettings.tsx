import { AppWindow, EyeOff, Power } from "lucide-react";

import { ToggleRow } from "@/components/ui/toggle-row";
import type { SettingsFormState } from "@/hooks/useSettings";

interface WindowSettingsProps {
  settings: SettingsFormState;
  onChange: (updates: Partial<SettingsFormState>) => void;
}

export function WindowSettings({ settings, onChange }: WindowSettingsProps) {
  return (
    <section className="space-y-4">
      <div className="flex items-center gap-2 border-b border-border-default pb-2">
        <AppWindow className="h-4 w-4 text-primary" />
        <h3 className="text-sm font-medium">窗口与启动</h3>
      </div>
      <div className="space-y-3">
        <ToggleRow
          icon={<Power className="h-4 w-4 text-emerald-600" />}
          title="开机启动"
          description="登录 Windows 后自动启动 WSL Code Switch。"
          checked={Boolean(settings.launchOnStartup)}
          onCheckedChange={(value) => onChange({ launchOnStartup: value })}
        />
        {settings.launchOnStartup ? (
          <ToggleRow
            icon={<EyeOff className="h-4 w-4 text-blue-600" />}
            title="静默启动"
            description="开机启动后保持窗口隐藏，只显示托盘图标。"
            checked={Boolean(settings.silentStartup)}
            onCheckedChange={(value) => onChange({ silentStartup: value })}
          />
        ) : null}
      </div>
    </section>
  );
}
