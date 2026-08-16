import { invoke } from "@tauri-apps/api/core";
import type { McpServer, McpServersMap } from "@/types";
import type { ManagedAppId } from "./types";

export const mcpApi = {
  async validateCommand(cmd: string): Promise<boolean> {
    return await invoke("validate_mcp_command", { cmd });
  },

  /**
   * 获取所有 MCP 服务器（统一结构）
   */
  async getAllServers(): Promise<McpServersMap> {
    return await invoke("get_mcp_servers");
  },

  /**
   * 添加或更新 MCP 服务器（统一结构）
   */
  async upsertUnifiedServer(server: McpServer): Promise<void> {
    return await invoke("upsert_mcp_server", { server });
  },

  /**
   * 删除 MCP 服务器
   */
  async deleteUnifiedServer(id: string): Promise<boolean> {
    return await invoke("delete_mcp_server", { id });
  },

  /**
   * 切换 MCP 服务器在指定应用的启用状态
   */
  async toggleApp(
    serverId: string,
    app: ManagedAppId,
    enabled: boolean,
  ): Promise<void> {
    return await invoke("toggle_mcp_app", { serverId, app, enabled });
  },

  /**
   * 从所有应用导入 MCP 服务器
   */
  async importFromApps(): Promise<number> {
    return await invoke("import_mcp_from_apps");
  },

  /** Explicitly project the application state to all managed live files. */
  async syncToApps(): Promise<void> {
    return await invoke("sync_mcp_to_apps");
  },
};
