import { useCallback, useEffect, useMemo, useState } from "react";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Form, FormField, FormItem, FormMessage } from "@/components/ui/form";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { Download, Loader2 } from "lucide-react";
import JsonEditor from "@/components/JsonEditor";
import { BasicFormFields } from "./BasicFormFields";
import { providerSchema, type ProviderFormData } from "@/lib/schemas/provider";
import type { ManagedAppId } from "@/lib/api";
import type { ProviderCategory, ProviderMeta } from "@/types";
import {
  fetchModelsForConfig,
  showFetchModelsError,
  type FetchedModel,
} from "@/lib/api/model-fetch";
import { resolveProviderModelFetchTarget } from "./providerModelFetch";
import { ProviderCommonSnippetSection } from "./ProviderCommonSnippetSection";

const CLAUDE_CUSTOM_CONFIG = {
  env: {
    ANTHROPIC_BASE_URL: "",
    ANTHROPIC_AUTH_TOKEN: "",
  },
};

const CODEX_CUSTOM_CONFIG = {
  auth: { OPENAI_API_KEY: "" },
  config:
    'model_provider = "custom"\n\n[model_providers.custom]\nname = "Custom"\nbase_url = "https://api.example.com/v1"\nwire_api = "responses"\nrequires_openai_auth = true\n',
};

const OPENCODE_CUSTOM_CONFIG = {
  npm: "@ai-sdk/openai-compatible",
  options: {
    baseURL: "https://api.example.com/v1",
    apiKey: "",
  },
  models: {},
};

const PROVIDER_KEY_PATTERN = /^[a-z0-9][a-z0-9._-]*$/;

function defaultConfig(appId: ManagedAppId): Record<string, unknown> {
  if (appId === "codex") return CODEX_CUSTOM_CONFIG;
  if (appId === "opencode") return OPENCODE_CUSTOM_CONFIG;
  return CLAUDE_CUSTOM_CONFIG;
}

function asObject(value: unknown): Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function parseJsonObject(
  value: string,
  label: string,
): Record<string, unknown> {
  const parsed: unknown = JSON.parse(value);
  if (parsed == null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label}必须是 JSON 对象`);
  }
  return parsed as Record<string, unknown>;
}

export interface ProviderFormProps {
  appId: ManagedAppId;
  providerId?: string;
  submitLabel: string;
  onSubmit: (values: ProviderFormValues) => Promise<void> | void;
  onCancel: () => void;
  onSubmittingChange?: (isSubmitting: boolean) => void;
  initialData?: {
    name?: string;
    websiteUrl?: string;
    notes?: string;
    settingsConfig?: Record<string, unknown>;
    category?: ProviderCategory;
    meta?: ProviderMeta;
    icon?: string;
    iconColor?: string;
  };
  showButtons?: boolean;
}

export function ProviderForm({
  appId,
  providerId,
  submitLabel,
  onSubmit,
  onCancel,
  onSubmittingChange,
  initialData,
  showButtons = true,
}: ProviderFormProps) {
  const { t } = useTranslation();
  const isEditMode = initialData != null;
  const initialConfig = useMemo(
    () => initialData?.settingsConfig ?? defaultConfig(appId),
    [appId, initialData?.settingsConfig],
  );
  const category: ProviderCategory =
    appId !== "opencode" && initialData?.category === "official"
      ? "official"
      : "custom";

  const form = useForm<ProviderFormData>({
    resolver: zodResolver(providerSchema),
    defaultValues: {
      name: initialData?.name ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      notes: initialData?.notes ?? "",
      settingsConfig: JSON.stringify(initialConfig, null, 2),
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
    },
    mode: "onSubmit",
  });

  const [providerKey, setProviderKey] = useState(providerId ?? "");
  const [codexAuth, setCodexAuth] = useState(() =>
    JSON.stringify(asObject(initialConfig.auth), null, 2),
  );
  const [codexToml, setCodexToml] = useState(() =>
    typeof initialConfig.config === "string" ? initialConfig.config : "",
  );
  const [codexError, setCodexError] = useState("");
  const [commonConfigEnabled, setCommonConfigEnabled] = useState(
    initialData?.meta?.commonConfigEnabled === true,
  );
  const [fetchedModels, setFetchedModels] = useState<FetchedModel[]>([]);
  const [isFetchingModels, setIsFetchingModels] = useState(false);
  const { isSubmitting } = form.formState;

  useEffect(() => {
    onSubmittingChange?.(isSubmitting);
  }, [isSubmitting, onSubmittingChange]);

  useEffect(() => {
    const nextConfig = initialData?.settingsConfig ?? defaultConfig(appId);
    form.reset({
      name: initialData?.name ?? "",
      websiteUrl: initialData?.websiteUrl ?? "",
      notes: initialData?.notes ?? "",
      settingsConfig: JSON.stringify(nextConfig, null, 2),
      icon: initialData?.icon ?? "",
      iconColor: initialData?.iconColor ?? "",
    });
    setProviderKey(providerId ?? "");
    setCodexAuth(JSON.stringify(asObject(nextConfig.auth), null, 2));
    setCodexToml(
      typeof nextConfig.config === "string" ? nextConfig.config : "",
    );
    setCodexError("");
    setCommonConfigEnabled(
      nextConfig != null && initialData?.meta?.commonConfigEnabled === true,
    );
    setFetchedModels([]);
  }, [appId, form, initialData, providerId]);

  const handleFetchModels = useCallback(async () => {
    let settingsConfig: Record<string, unknown>;
    try {
      settingsConfig =
        appId === "codex"
          ? {
              ...initialConfig,
              auth: parseJsonObject(codexAuth, "auth.json"),
              config: codexToml,
            }
          : parseJsonObject(form.getValues("settingsConfig"), "配置");
    } catch (error) {
      showFetchModelsError(error, t);
      return;
    }

    const target = resolveProviderModelFetchTarget(
      appId,
      settingsConfig,
      initialData?.meta,
    );
    if (!target.baseUrl || !target.apiKey) {
      showFetchModelsError(null, t, {
        hasApiKey: Boolean(target.apiKey),
        hasBaseUrl: Boolean(target.baseUrl),
      });
      return;
    }

    setIsFetchingModels(true);
    try {
      const models = await fetchModelsForConfig(
        target.baseUrl,
        target.apiKey,
        target.isFullUrl,
        target.modelsUrl,
        target.customUserAgent,
      );
      setFetchedModels(models);
      if (models.length === 0) {
        toast.info(t("providerForm.fetchModelsEmpty"));
      } else {
        toast.success(
          t("providerForm.fetchModelsSuccess", { count: models.length }),
        );
      }
    } catch (error) {
      showFetchModelsError(error, t);
    } finally {
      setIsFetchingModels(false);
    }
  }, [appId, codexAuth, codexToml, form, initialConfig, initialData?.meta, t]);

  const submit = form.handleSubmit(async (values) => {
    if (!values.name.trim()) {
      toast.error(
        t("provider.nameRequired", { defaultValue: "请输入供应商名称" }),
      );
      return;
    }

    if (appId === "opencode") {
      const key = providerKey.trim();
      if (!key) {
        toast.error(
          t("opencode.providerKeyRequired", {
            defaultValue: "请输入 OpenCode Provider Key",
          }),
        );
        return;
      }
      if (!PROVIDER_KEY_PATTERN.test(key)) {
        toast.error(
          t("opencode.providerKeyInvalid", {
            defaultValue:
              "Provider Key 只能包含小写字母、数字、点、下划线和连字符",
          }),
        );
        return;
      }
    }

    let settingsConfig = values.settingsConfig;
    if (appId === "codex") {
      try {
        const auth = parseJsonObject(codexAuth, "auth.json");
        const extras = { ...initialConfig };
        delete extras.auth;
        delete extras.config;
        settingsConfig = JSON.stringify({
          ...extras,
          auth,
          config: codexToml,
        });
        setCodexError("");
      } catch (error) {
        setCodexError(error instanceof Error ? error.message : String(error));
        return;
      }
    }

    const meta: ProviderMeta = { ...(initialData?.meta ?? {}) };
    if (appId === "opencode") {
      delete meta.commonConfigEnabled;
    } else {
      meta.commonConfigEnabled = commonConfigEnabled;
    }

    await onSubmit({
      ...values,
      settingsConfig,
      presetCategory: category,
      meta,
      ...(appId === "opencode" ? { providerKey: providerKey.trim() } : {}),
    });
  });

  return (
    <Form {...form}>
      <form id="provider-form" onSubmit={submit} className="space-y-6">
        <BasicFormFields
          form={form}
          beforeNameSlot={
            appId === "opencode" ? (
              <div className="space-y-2">
                <Label htmlFor="providerKey">
                  {t("opencode.providerKey", { defaultValue: "Provider Key" })}
                </Label>
                <Input
                  id="providerKey"
                  value={providerKey}
                  onChange={(event) => setProviderKey(event.target.value)}
                  disabled={isEditMode}
                  placeholder="my-provider"
                  autoComplete="off"
                />
                <p className="text-xs text-muted-foreground">
                  {isEditMode
                    ? t("opencode.providerKeyLockedHint", {
                        defaultValue: "Provider Key 创建后不可修改",
                      })
                    : t("opencode.providerKeyHint", {
                        defaultValue: "该键将直接写入 OpenCode provider 配置",
                      })}
                </p>
              </div>
            ) : undefined
          }
        />

        {appId === "codex" ? (
          <div className="space-y-5">
            <div className="space-y-2">
              <Label htmlFor="codex-auth">auth.json</Label>
              <JsonEditor
                value={codexAuth}
                onChange={setCodexAuth}
                language="json"
                rows={5}
                showValidation
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="codex-config">config.toml</Label>
              <Textarea
                id="codex-config"
                value={codexToml}
                onChange={(event) => setCodexToml(event.target.value)}
                className="min-h-56 font-mono"
              />
            </div>
            {codexError && (
              <p className="text-sm text-destructive">{codexError}</p>
            )}
          </div>
        ) : (
          <div className="space-y-2">
            <Label htmlFor="settingsConfig">
              {appId === "claude"
                ? "Claude settings.json"
                : "OpenCode provider JSON"}
            </Label>
            <JsonEditor
              value={form.watch("settingsConfig")}
              onChange={(value) =>
                form.setValue("settingsConfig", value, { shouldValidate: true })
              }
              language="json"
              rows={12}
              showValidation
            />
            <FormField
              control={form.control}
              name="settingsConfig"
              render={() => (
                <FormItem>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        )}

        {appId !== "opencode" && (
          <ProviderCommonSnippetSection
            appId={appId}
            enabled={commonConfigEnabled}
            onEnabledChange={setCommonConfigEnabled}
          />
        )}

        {category !== "official" && (
          <div className="space-y-3 border-t border-border pt-4">
            <Button
              type="button"
              variant="outline"
              onClick={handleFetchModels}
              disabled={isFetchingModels}
            >
              {isFetchingModels ? (
                <Loader2 className="mr-2 h-4 w-4 animate-spin" />
              ) : (
                <Download className="mr-2 h-4 w-4" />
              )}
              {t(
                isFetchingModels
                  ? "providerForm.fetchingModels"
                  : "providerForm.fetchModels",
              )}
            </Button>
            {fetchedModels.length > 0 && (
              <div
                className="max-h-36 overflow-y-auto rounded-md border border-border bg-muted/30 p-3"
                aria-label={t("providerForm.fetchedModelsLabel", {
                  defaultValue: "已获取的模型",
                })}
              >
                <div className="flex flex-wrap gap-2">
                  {fetchedModels.map((model) => (
                    <code
                      key={model.id}
                      className="rounded border border-border bg-background px-2 py-1 text-xs"
                    >
                      {model.id}
                    </code>
                  ))}
                </div>
              </div>
            )}
          </div>
        )}

        {showButtons && (
          <div className="flex justify-end gap-2">
            <Button type="button" variant="outline" onClick={onCancel}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={isSubmitting}>
              {submitLabel}
            </Button>
          </div>
        )}
      </form>
    </Form>
  );
}

export type ProviderFormValues = ProviderFormData & {
  presetId?: string;
  presetCategory?: ProviderCategory;
  meta?: ProviderMeta;
  providerKey?: string;
};
