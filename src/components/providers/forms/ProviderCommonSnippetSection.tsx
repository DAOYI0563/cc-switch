import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download, Loader2, Save } from "lucide-react";
import { configApi } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

type CommonSnippetAppId = "claude" | "codex";

interface ProviderCommonSnippetSectionProps {
  appId: CommonSnippetAppId;
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
}

export function ProviderCommonSnippetSection({
  appId,
  enabled,
  onEnabledChange,
}: ProviderCommonSnippetSectionProps) {
  const { t } = useTranslation();
  const [snippet, setSnippet] = useState("");
  const [error, setError] = useState("");
  const [isLoading, setIsLoading] = useState(true);
  const [isExtracting, setIsExtracting] = useState(false);
  const [isSaving, setIsSaving] = useState(false);

  useEffect(() => {
    let active = true;
    setIsLoading(true);
    setError("");
    configApi
      .getCommonConfigSnippet(appId)
      .then((value) => {
        if (active) setSnippet(value ?? "");
      })
      .catch((loadError: unknown) => {
        if (active) setError(String(loadError));
      })
      .finally(() => {
        if (active) setIsLoading(false);
      });
    return () => {
      active = false;
    };
  }, [appId]);

  const extractFromLive = useCallback(async () => {
    setIsExtracting(true);
    setError("");
    try {
      setSnippet(await configApi.extractCommonConfigSnippet(appId));
    } catch (extractError) {
      setError(String(extractError));
    } finally {
      setIsExtracting(false);
    }
  }, [appId]);

  const saveSnippet = useCallback(async () => {
    setIsSaving(true);
    setError("");
    try {
      await configApi.setCommonConfigSnippet(appId, snippet);
    } catch (saveError) {
      setError(String(saveError));
    } finally {
      setIsSaving(false);
    }
  }, [appId, snippet]);

  return (
    <section className="space-y-3 border-t border-border pt-4">
      <div className="flex min-h-9 items-center justify-between gap-4">
        <Label htmlFor={`${appId}-common-snippet`}>
          {t("providerForm.commonConfig.title")}
        </Label>
        <div className="flex items-center gap-2">
          <span className="text-sm text-muted-foreground">
            {t("providerForm.commonConfig.apply")}
          </span>
          <Switch
            aria-label={t("providerForm.commonConfig.apply")}
            checked={enabled}
            onCheckedChange={onEnabledChange}
          />
        </div>
      </div>

      <Textarea
        id={`${appId}-common-snippet`}
        value={snippet}
        onChange={(event) => setSnippet(event.target.value)}
        disabled={isLoading}
        className="min-h-40 font-mono text-xs"
        spellCheck={false}
      />

      <div className="flex flex-wrap justify-end gap-2">
        <Button
          type="button"
          variant="outline"
          onClick={extractFromLive}
          disabled={isLoading || isExtracting || isSaving}
        >
          {isExtracting ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Download className="h-4 w-4" />
          )}
          {t("providerForm.commonConfig.extract")}
        </Button>
        <Button
          type="button"
          variant="outline"
          onClick={saveSnippet}
          disabled={isLoading || isExtracting || isSaving}
        >
          {isSaving ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <Save className="h-4 w-4" />
          )}
          {t("providerForm.commonConfig.save")}
        </Button>
      </div>

      {error && <p className="text-sm text-destructive">{error}</p>}
    </section>
  );
}
