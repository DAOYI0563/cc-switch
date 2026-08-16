import type { ComponentType, MouseEvent, ReactNode } from "react";
import { Monitor, Moon, Sun } from "lucide-react";

import { useTheme } from "@/components/theme-provider";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function ThemeSettings() {
  const { theme, setTheme } = useTheme();
  return (
    <section className="space-y-2">
      <header>
        <h3 className="text-sm font-medium">主题</h3>
        <p className="mt-1 text-xs text-muted-foreground">
          选择浅色、深色或跟随 Windows。
        </p>
      </header>
      <div className="inline-flex gap-1 rounded-md border border-border-default p-1">
        <ThemeButton
          active={theme === "light"}
          onClick={() => setTheme("light")}
          icon={Sun}
        >
          浅色
        </ThemeButton>
        <ThemeButton
          active={theme === "dark"}
          onClick={() => setTheme("dark")}
          icon={Moon}
        >
          深色
        </ThemeButton>
        <ThemeButton
          active={theme === "system"}
          onClick={() => setTheme("system")}
          icon={Monitor}
        >
          跟随系统
        </ThemeButton>
      </div>
    </section>
  );
}

function ThemeButton({
  active,
  onClick,
  icon: Icon,
  children,
}: {
  active: boolean;
  onClick: (event: MouseEvent<HTMLButtonElement>) => void;
  icon: ComponentType<{ className?: string }>;
  children: ReactNode;
}) {
  return (
    <Button
      type="button"
      size="sm"
      variant={active ? "default" : "ghost"}
      onClick={onClick}
      className={cn("min-w-[88px] gap-1.5", !active && "text-muted-foreground")}
    >
      <Icon className="h-3.5 w-3.5" />
      {children}
    </Button>
  );
}
