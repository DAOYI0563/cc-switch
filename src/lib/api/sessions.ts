import { invoke } from "@tauri-apps/api/core";
import type {
  SessionMessage,
  SessionMeta,
  SessionPage,
  SessionSearchRequest,
} from "@/types";

export const sessionsApi = {
  async list(): Promise<SessionMeta[]> {
    return await invoke("list_sessions");
  },

  async search(
    request: SessionSearchRequest,
  ): Promise<SessionPage<SessionMeta>> {
    return await invoke("search_sessions", { request });
  },

  async getMessages(
    providerId: string,
    sessionId: string,
    offset = 0,
    limit = 200,
  ): Promise<SessionPage<SessionMessage>> {
    return await invoke("get_session_messages", {
      providerId,
      sessionId,
      offset,
      limit,
    });
  },

  async launchTerminal(options: {
    providerId: string;
    sessionId: string;
  }): Promise<boolean> {
    return await invoke("launch_session_terminal", options);
  },
};
