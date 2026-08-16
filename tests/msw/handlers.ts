import { http, HttpResponse } from "msw";
import type { ManagedAppId } from "@/lib/api/types";
import type { McpServer, Provider, Settings } from "@/types";
import {
  addProvider,
  deleteMcpServer,
  deleteProvider,
  getCurrentProviderId,
  getMcpConfig,
  getOpenCodeLiveProviderIds,
  getProviders,
  getSessionMessages,
  getSettings,
  listProviders,
  listSessions,
  resetProviderState,
  setCurrentProviderId,
  setMcpServerEnabled,
  setSettings,
  updateProvider,
  updateSortOrder,
  upsertMcpServer,
} from "./state";

const TAURI_ENDPOINT = "http://tauri.local";

const readJson = async <T>(request: Request): Promise<T> => {
  const body = await request.text();
  return body ? (JSON.parse(body) as T) : ({} as T);
};

const success = <T>(payload: T) => HttpResponse.json(payload as never);

export const handlers = [
  http.post(`${TAURI_ENDPOINT}/local_scan_enter_page`, () => success(null)),
  http.post(`${TAURI_ENDPOINT}/list_conflict_center_items_command`, () =>
    success([]),
  ),
  http.post(`${TAURI_ENDPOINT}/resolve_conflict_center_item_command`, () =>
    success(null),
  ),
  http.post(`${TAURI_ENDPOINT}/get_migration_result`, () => success(false)),
  http.post(`${TAURI_ENDPOINT}/get_common_config_snippet`, () => success(null)),
  http.post(`${TAURI_ENDPOINT}/set_common_config_snippet`, () => success(null)),
  http.post(`${TAURI_ENDPOINT}/extract_common_config_snippet`, () => success("")),
  http.post(`${TAURI_ENDPOINT}/get_providers`, async ({ request }) => {
    const { app } = await readJson<{ app: ManagedAppId }>(request);
    return success(getProviders(app));
  }),
  http.post(`${TAURI_ENDPOINT}/get_current_provider`, async ({ request }) => {
    const { app } = await readJson<{ app: ManagedAppId }>(request);
    return success(getCurrentProviderId(app));
  }),
  http.post(
    `${TAURI_ENDPOINT}/update_providers_sort_order`,
    async ({ request }) => {
      const { app, updates = [] } = await readJson<{
        app: ManagedAppId;
        updates: { id: string; sortIndex: number }[];
      }>(request);
      updateSortOrder(app, updates);
      return success(true);
    },
  ),
  http.post(`${TAURI_ENDPOINT}/update_tray_menu`, () => success(true)),
  http.post(`${TAURI_ENDPOINT}/get_opencode_live_provider_ids`, () =>
    success(getOpenCodeLiveProviderIds()),
  ),
  http.post(`${TAURI_ENDPOINT}/switch_provider`, async ({ request }) => {
    const { app, id } = await readJson<{ app: ManagedAppId; id: string }>(request);
    if (!listProviders(app)[id]) return HttpResponse.json(false, { status: 404 });
    setCurrentProviderId(app, id);
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/add_provider`, async ({ request }) => {
    const { app, provider } = await readJson<{
      app: ManagedAppId;
      provider: Provider & { id?: string };
    }>(request);
    addProvider(app, { ...provider, id: provider.id ?? `mock-${Date.now()}` });
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/update_provider`, async ({ request }) => {
    const { app, provider } = await readJson<{
      app: ManagedAppId;
      provider: Provider;
    }>(request);
    updateProvider(app, provider);
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/delete_provider`, async ({ request }) => {
    const { app, id } = await readJson<{ app: ManagedAppId; id: string }>(request);
    deleteProvider(app, id);
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/import_default_config`, () => {
    resetProviderState();
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/import_opencode_from_live`, () => success(0)),
  http.post(`${TAURI_ENDPOINT}/remove_opencode_provider_from_live`, () =>
    success(true),
  ),
  http.post(`${TAURI_ENDPOINT}/open_external`, () => success(true)),
  http.post(`${TAURI_ENDPOINT}/list_sessions`, () => success(listSessions())),
  http.post(`${TAURI_ENDPOINT}/search_sessions`, async ({ request }) => {
    const { request: query } = await readJson<{
      request?: { offset?: number; limit?: number };
    }>(request);
    const all = listSessions();
    const offset = query?.offset ?? 0;
    const limit = query?.limit ?? 50;
    return success({
      items: all.slice(offset, offset + limit),
      offset,
      total: all.length,
      nextOffset: offset + limit < all.length ? offset + limit : undefined,
    });
  }),
  http.post(`${TAURI_ENDPOINT}/get_session_messages`, async ({ request }) => {
    const body = await readJson<{
      providerId: string;
      sessionId: string;
      offset?: number;
      limit?: number;
    }>(request);
    const all = getSessionMessages(body.providerId, body.sessionId);
    const offset = body.offset ?? 0;
    const limit = body.limit ?? 200;
    return success({
      items: all.slice(offset, offset + limit),
      offset,
      total: all.length,
      nextOffset: offset + limit < all.length ? offset + limit : undefined,
    });
  }),
  http.post(`${TAURI_ENDPOINT}/get_mcp_config`, async ({ request }) => {
    const { app } = await readJson<{ app: ManagedAppId }>(request);
    return success(getMcpConfig(app));
  }),
  http.post(`${TAURI_ENDPOINT}/import_mcp_from_claude`, () => success(1)),
  http.post(`${TAURI_ENDPOINT}/import_mcp_from_codex`, () => success(1)),
  http.post(`${TAURI_ENDPOINT}/set_mcp_enabled`, async ({ request }) => {
    const { app, id, enabled } = await readJson<{
      app: ManagedAppId;
      id: string;
      enabled: boolean;
    }>(request);
    setMcpServerEnabled(app, id, enabled);
    return success(true);
  }),
  http.post(
    `${TAURI_ENDPOINT}/upsert_mcp_server_in_config`,
    async ({ request }) => {
      const { app, id, spec } = await readJson<{
        app: ManagedAppId;
        id: string;
        spec: McpServer;
      }>(request);
      upsertMcpServer(app, id, spec);
      return success(true);
    },
  ),
  http.post(
    `${TAURI_ENDPOINT}/delete_mcp_server_in_config`,
    async ({ request }) => {
      const { app, id } = await readJson<{ app: ManagedAppId; id: string }>(
        request,
      );
      deleteMcpServer(app, id);
      return success(true);
    },
  ),
  http.post(`${TAURI_ENDPOINT}/get_settings`, () => success(getSettings())),
  http.post(`${TAURI_ENDPOINT}/save_settings`, async ({ request }) => {
    const { settings } = await readJson<{ settings: Settings }>(request);
    setSettings(settings);
    return success(true);
  }),
  http.post(`${TAURI_ENDPOINT}/get_config_dir`, async ({ request }) => {
    const { app } = await readJson<{ app: ManagedAppId }>(request);
    return success(`/mock/${app}`);
  }),
  http.post(`${TAURI_ENDPOINT}/is_portable_mode`, () => success(false)),
];
